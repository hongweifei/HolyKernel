//! 任务句柄表：额外句柄、能力、Region 与 DeviceGrant。

use super::*;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    pub(crate) fn install_handle(&mut self, index: usize, message: &Message) -> u64 {
        let Some(handle) = message.handles[0] else {
            return 0;
        };
        self.install_extra_capability(index, handle).unwrap_or(0)
    }

    /// 安装一个额外能力句柄并记录容量记账。
    pub(crate) fn install_extra_capability(&mut self, index: usize, handle: Handle) -> Option<u64> {
        let tcb = self.tasks[index].tcb;
        let Ok((capability, _)) = self.kernel.capability(tcb, handle) else {
            // Frame 句柄没有类型化能力对象；接收方按物理区间大小记账，
            // 否则 MEMORY_FRAME 之类的转移会完全绕过接收任务的配额。
            let Ok((range, _, _, _, _)) = self.kernel.frame_info(tcb, handle) else {
                return Some(self.install_extra_handle(index, handle));
            };
            let Ok(charge) = self.process_mut(index).ledger.charge(range.len()) else {
                let _ = self.kernel.close_handle(tcb, handle);
                return None;
            };
            return Some(self.install_extra_entry(index, handle, Some(charge)));
        };
        let Ok(charge) = self.process_mut(index).ledger.charge(capability.size()) else {
            let _ = self.kernel.close_handle(tcb, handle);
            return None;
        };
        Some(self.install_extra_entry(index, handle, Some(charge)))
    }

    /// 把装载器已映射的镜像段与用户栈登记为 Region。
    pub(crate) fn register_user_regions(
        &mut self,
        index: usize,
        specs: &[UserRegionSpec],
    ) -> Result<(), KernelError> {
        for spec in specs {
            // 镜像与用户栈页由装载器从任务 Capacity 分配；虚拟区间同样计入
            // Task Ledger，配额不能只约束 `SYS_ALLOC`。
            let charge = self
                .process_mut(index)
                .ledger
                .charge(spec.len as u64)
                .map_err(KernelError::Hmp)?;
            self.register_region(index, *spec, Some(charge))?;
        }
        Ok(())
    }

    /// 把一个已物化的用户映射登记为 Region。
    pub(crate) fn register_region(
        &mut self,
        index: usize,
        spec: UserRegionSpec,
        charge: Option<Charge>,
    ) -> Result<(), KernelError> {
        if spec.len == 0 {
            return Ok(());
        }
        let tcb = self.tasks[index].tcb;
        let mut rights = Rights::READ;
        if spec.writable {
            rights |= Rights::WRITE;
        }
        if spec.executable {
            rights |= Rights::EXECUTE;
        }
        let flags = hk_mem::hmp::RegionFlags::empty();
        let kind = if spec.executable {
            hk_mem::hmp::CapabilityKind::TaskImage
        } else {
            hk_mem::hmp::CapabilityKind::Anonymous
        };
        let capability =
            hk_mem::hmp::Capability::new(kind, spec.len as u64, hk_hal::PAGE_SIZE, None)
                .map_err(KernelError::Hmp)?;
        let capability_handle = self.kernel.create_capability(
            tcb,
            capability.clone(),
            None,
            Rights::READ | Rights::WRITE | Rights::ADMIN,
        )?;
        let mut region = match hk_mem::hmp::Region::new(&capability, 0, spec.len, rights, flags) {
            Ok(region) => region,
            Err(error) => {
                let _ = self.kernel.close_handle(tcb, capability_handle);
                return Err(KernelError::Hmp(error));
            }
        };
        if let Err(error) = region.map(hk_hal::VirtAddr::new(spec.virt)) {
            let _ = self.kernel.close_handle(tcb, capability_handle);
            return Err(KernelError::Hmp(error));
        }
        let region_id = region.id();
        if self.process_mut(index).hmp_arena.insert(&region).is_err() {
            let _ = self.kernel.close_handle(tcb, capability_handle);
            return Err(KernelError::Hmp(hk_mem::hmp::HmpError::AlreadyMapped));
        }
        let region_handle = match self.kernel.create_region(
            tcb,
            region,
            Rights::READ | Rights::WRITE | Rights::ADMIN,
        ) {
            Ok(handle) => handle,
            Err(error) => {
                let _ = self.process_mut(index).hmp_arena.remove(region_id);
                let _ = self.kernel.close_handle(tcb, capability_handle);
                return Err(error);
            }
        };
        self.install_region(
            index,
            region_handle,
            capability_handle,
            region_id,
            true,
            spec.virt,
            spec.len,
            0,
            rights,
            flags,
            charge,
        );
        Ok(())
    }

    /// 复制父任务的 capability/Region 元数据到 fork 子任务。
    pub(crate) fn fork_regions(&mut self, parent: usize, child: usize) {
        type ForkRegionEntry = (Handle, Handle, u64, usize, u64, hk_mem::hmp::RegionFlags);
        let entries: Vec<ForkRegionEntry> = self
            .process(parent)
            .regions
            .iter()
            .flatten()
            .map(|region| {
                (
                    region.handle,
                    region.capability,
                    region.virt,
                    region.len,
                    region.offset,
                    region.flags,
                )
            })
            .collect();
        let parent_tcb = self.tasks[parent].tcb;
        let child_tcb = self.tasks[child].tcb;
        for (region_handle, capability_handle, virt, len, offset, flags) in entries {
            if flags.contains(hk_mem::hmp::RegionFlags::DONT_FORK) {
                // 子地址空间在 fork 时已复制 PTE；DONT_FORK 要求在子侧解除。
                let _ = self.platform.unmap_user(child, virt, len);
                continue;
            }
            let Ok(capability_rights) = self.kernel.handle_rights(
                parent_tcb,
                capability_handle,
                hk_handle::ObjectKind::Capability,
            ) else {
                continue;
            };
            let Ok(region_handle_rights) =
                self.kernel
                    .handle_rights(parent_tcb, region_handle, hk_handle::ObjectKind::Region)
            else {
                continue;
            };
            let Ok(capability_id) = self.kernel.handle_id(parent_tcb, capability_handle) else {
                continue;
            };
            // 子任务需要自己的 capability 句柄；duplicate 只作用于同一 TCB。
            let Ok(capability) = self
                .kernel
                .grant(child_tcb, capability_id, capability_rights)
            else {
                continue;
            };
            let Ok(region) =
                self.kernel
                    .fork_region(parent_tcb, region_handle, hk_hal::VirtAddr::new(virt))
            else {
                let _ = self.kernel.close_handle(child_tcb, capability);
                continue;
            };
            let _ = self.kernel.mark_region_cow(parent_tcb, region_handle);
            let region_rights = region.rights();
            let region_flags = region.flags();
            let region_id = region.id();
            if self.process_mut(child).hmp_arena.insert(&region).is_err() {
                let _ = self.kernel.close_handle(child_tcb, capability);
                continue;
            }
            let Ok(child_region) =
                self.kernel
                    .create_region(child_tcb, region, region_handle_rights)
            else {
                let _ = self.kernel.close_handle(child_tcb, capability);
                let _ = self.process_mut(child).hmp_arena.remove(region_id);
                continue;
            };
            // 子任务的自动窗口必须知道这段继承映射已经占用。
            let _ = self.process_mut(child).user_window.reserve(virt, len);
            let Ok((_, size)) = self
                .kernel
                .capability(child_tcb, capability)
                .map(|(capability, _)| (capability.clone(), capability.size()))
            else {
                let _ = self.kernel.close_handle(child_tcb, capability);
                let _ = self.kernel.close_handle(child_tcb, child_region);
                continue;
            };
            let Ok(charge) = self.process_mut(child).ledger.charge(size) else {
                let _ = self.kernel.close_handle(child_tcb, capability);
                let _ = self.kernel.close_handle(child_tcb, child_region);
                continue;
            };
            self.install_capability(child, capability, charge);
            self.install_region(
                child,
                child_region,
                capability,
                region_id,
                false,
                virt,
                len,
                offset,
                region_rights,
                region_flags,
                None,
            );
        }
    }

    pub(crate) fn install_extra_handle(&mut self, index: usize, handle: Handle) -> u64 {
        self.install_extra_entry(index, handle, None)
    }

    fn install_extra_entry(&mut self, index: usize, handle: Handle, charge: Option<Charge>) -> u64 {
        let slot = match self.tasks[index].extra_free.pop() {
            Some(slot) => slot,
            None => {
                self.tasks[index].extra.push(None);
                self.tasks[index].extra.len() - 1
            }
        };
        self.tasks[index].extra[slot] = Some(HandleSlot { handle, charge });
        (EXTRA_HANDLE_BASE + slot + 1) as u64
    }

    pub(crate) fn install_capability(
        &mut self,
        index: usize,
        handle: Handle,
        charge: Charge,
    ) -> u64 {
        let slot = match self.tasks[index].capability_free.pop() {
            Some(slot) => slot,
            None => {
                self.tasks[index].capabilities.push(None);
                self.tasks[index].capabilities.len() - 1
            }
        };
        self.tasks[index].capabilities[slot] = Some(CapabilityMap { handle, charge });
        (CAPABILITY_HANDLE_BASE + slot + 1) as u64
    }

    pub(crate) fn capability_at(
        &self,
        index: usize,
        number: usize,
    ) -> Option<(Handle, hk_mem::hmp::Capability, Option<hk_hal::PhysRange>)> {
        let handle = if (CAPABILITY_HANDLE_BASE..REGION_HANDLE_BASE).contains(&number) {
            self.tasks[index]
                .capabilities
                .get(number - CAPABILITY_HANDLE_BASE - 1)?
                .as_ref()?
                .handle
        } else {
            self.tasks[index]
                .extra
                .get(number.checked_sub(EXTRA_HANDLE_BASE + 1)?)?
                .as_ref()?
                .handle
        };
        let (capability, backing) = self.kernel.capability(self.tasks[index].tcb, handle).ok()?;
        Some((handle, capability, backing))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn install_region(
        &mut self,
        index: usize,
        handle: Handle,
        capability: Handle,
        region: hk_mem::hmp::RegionId,
        internal_capability: bool,
        virt: u64,
        len: usize,
        offset: u64,
        rights: Rights,
        flags: hk_mem::hmp::RegionFlags,
        charge: Option<Charge>,
    ) -> u64 {
        let slot = match self.process_mut(index).region_free.pop() {
            Some(slot) => slot,
            None => {
                let process = self.process_mut(index);
                process.regions.push(None);
                process.regions.len() - 1
            }
        };
        let map = RegionMap {
            handle,
            capability,
            owner: self.tasks[index].tcb,
            region,
            internal_capability,
            virt,
            len,
            offset,
            rights,
            flags,
            charge,
        };
        self.process_mut(index).regions[slot] = Some(map);
        (REGION_HANDLE_BASE + slot + 1) as u64
    }

    pub(crate) fn region_at(
        &self,
        index: usize,
        number: usize,
    ) -> Option<(usize, Handle, u64, usize)> {
        let slot = number.checked_sub(REGION_HANDLE_BASE + 1)?;
        let map = self.process(index).regions.get(slot)?.as_ref()?;
        Some((slot, map.handle, map.virt, map.len))
    }

    pub(crate) fn install_capacity(&mut self, index: usize, handle: Handle) -> u64 {
        let slot = match self.tasks[index].capacity_free.pop() {
            Some(slot) => slot,
            None => {
                self.tasks[index].capacities.push(None);
                self.tasks[index].capacities.len() - 1
            }
        };
        self.tasks[index].capacities[slot] = Some(handle);
        (CAPACITY_HANDLE_BASE + slot + 1) as u64
    }

    pub(crate) fn capacity_at(&self, index: usize, number: usize) -> Option<(usize, Handle)> {
        let slot = number.checked_sub(CAPACITY_HANDLE_BASE + 1)?;
        let handle = self.tasks[index].capacities.get(slot)?.as_ref()?;
        Some((slot, *handle))
    }

    pub(crate) fn install_frame(
        &mut self,
        index: usize,
        handle: Handle,
        charge: Option<Charge>,
    ) -> u64 {
        let slot = match self.tasks[index].frame_free.pop() {
            Some(slot) => slot,
            None => {
                self.tasks[index].frames.push(None);
                self.tasks[index].frames.len() - 1
            }
        };
        self.tasks[index].frames[slot] = Some(HandleSlot { handle, charge });
        (FRAME_HANDLE_BASE + slot + 1) as u64
    }

    pub(crate) fn frame_at(&self, index: usize, number: usize) -> Option<(usize, Handle)> {
        let slot = number.checked_sub(FRAME_HANDLE_BASE + 1)?;
        let entry = self.tasks[index].frames.get(slot)?.as_ref()?;
        Some((slot, entry.handle))
    }

    /// 取出指定 Frame 槽位的容量记账，供映射或释放转移。
    pub(crate) fn take_frame_charge(&mut self, index: usize, number: usize) -> Option<Charge> {
        if number >= FRAME_HANDLE_BASE {
            let slot = number.checked_sub(FRAME_HANDLE_BASE + 1)?;
            self.tasks[index]
                .frames
                .get_mut(slot)?
                .as_mut()?
                .charge
                .take()
        } else if number >= EXTRA_HANDLE_BASE {
            let slot = number.checked_sub(EXTRA_HANDLE_BASE + 1)?;
            self.tasks[index]
                .extra
                .get_mut(slot)?
                .as_mut()?
                .charge
                .take()
        } else {
            None
        }
    }

    /// 归还当前线程持有的全部额外记账；任务退出时调用。
    pub(crate) fn release_task_charges(&mut self, index: usize) {
        let frames = core::mem::take(&mut self.tasks[index].frames);
        let capabilities = core::mem::take(&mut self.tasks[index].capabilities);
        let extra = core::mem::take(&mut self.tasks[index].extra);
        self.tasks[index].frame_free.clear();
        self.tasks[index].capability_free.clear();
        self.tasks[index].extra_free.clear();
        for slot in frames.into_iter().flatten() {
            if let Some(charge) = slot.charge {
                let _ = self.process_mut(index).ledger.release(charge);
            }
        }
        for map in capabilities.into_iter().flatten() {
            let _ = self.process_mut(index).ledger.release(map.charge);
        }
        for slot in extra.into_iter().flatten() {
            if let Some(charge) = slot.charge {
                let _ = self.process_mut(index).ledger.release(charge);
            }
        }
    }

    pub(crate) fn grant_at(&self, index: usize, number: usize) -> Option<Handle> {
        if let Some(entry) = number
            .checked_sub(GRANT_HANDLE_BASE + 1)
            .and_then(|slot| self.tasks[index].grants.get(slot))
            .and_then(|slot| slot.as_ref())
        {
            return Some(entry.handle);
        }
        let handle = self.tasks[index]
            .extra
            .get(number.checked_sub(EXTRA_HANDLE_BASE + 1)?)
            .and_then(|slot| slot.as_ref())?
            .handle;
        self.kernel
            .device_grant(self.tasks[index].tcb, handle)
            .ok()?;
        Some(handle)
    }

    pub(crate) fn install_grant(
        &mut self,
        index: usize,
        handle: Handle,
        charge: Option<hk_mem::hmp::Charge>,
    ) -> u64 {
        let slot = self.tasks[index]
            .grants
            .iter()
            .position(Option::is_none)
            .unwrap_or(self.tasks[index].grants.len());
        let entry = HandleSlot { handle, charge };
        if slot == self.tasks[index].grants.len() {
            self.tasks[index].grants.push(Some(entry));
        } else {
            self.tasks[index].grants[slot] = Some(entry);
        }
        (GRANT_HANDLE_BASE + slot + 1) as u64
    }

    pub(crate) fn handle_at(&self, index: usize, number: usize) -> Option<Handle> {
        if number >= FRAME_HANDLE_BASE {
            self.frame_at(index, number).map(|(_, handle)| handle)
        } else if number >= CAPACITY_HANDLE_BASE {
            self.capacity_at(index, number).map(|(_, handle)| handle)
        } else if number >= REGION_HANDLE_BASE {
            self.region_at(index, number)
                .map(|(_, handle, _, _)| handle)
        } else if number >= CAPABILITY_HANDLE_BASE {
            self.capability_at(index, number)
                .map(|(handle, _, _)| handle)
        } else if number >= GRANT_HANDLE_BASE {
            self.grant_at(index, number)
        } else if number >= EXTRA_HANDLE_BASE {
            self.tasks[index]
                .extra
                .get(number - EXTRA_HANDLE_BASE - 1)
                .and_then(|slot| slot.as_ref())
                .map(|slot| slot.handle)
        } else {
            self.tasks[index]
                .handles
                .get(number.wrapping_sub(1))
                .copied()
                .flatten()
        }
    }

    /// 返回进程常驻字节数：各能力后备区间与已装入页之和。
    pub(crate) fn process_resident(&self, index: usize) -> u64 {
        let tcb = self.tasks[index].tcb;
        let mut total: u64 = 0;
        for map in self.tasks[index].capabilities.iter().flatten() {
            total = total.saturating_add(
                self.kernel
                    .capability_resident(tcb, map.handle)
                    .unwrap_or(0),
            );
        }
        for slot in self.tasks[index].extra.iter().flatten() {
            total = total.saturating_add(
                self.kernel
                    .capability_resident(tcb, slot.handle)
                    .unwrap_or(0),
            );
        }
        for region in self.process(index).regions.iter().flatten() {
            if region.internal_capability {
                total = total.saturating_add(
                    self.kernel
                        .capability_resident(tcb, region.capability)
                        .unwrap_or(0),
                );
            }
        }
        total
    }

    pub(crate) fn clear_handle(&mut self, index: usize, number: usize) {
        if number >= FRAME_HANDLE_BASE {
            if let Some(slot_index) = number.checked_sub(FRAME_HANDLE_BASE + 1)
                && let Some(entry) = self.tasks[index]
                    .frames
                    .get_mut(slot_index)
                    .and_then(|slot| slot.take())
            {
                self.tasks[index].frame_free.push(slot_index);
                if let Some(charge) = entry.charge {
                    let _ = self.process_mut(index).ledger.release(charge);
                }
            }
        } else if number >= CAPACITY_HANDLE_BASE {
            if let Some(slot_index) = number.checked_sub(CAPACITY_HANDLE_BASE + 1)
                && let Some(slot) = self.tasks[index].capacities.get_mut(slot_index)
                && slot.is_some()
            {
                *slot = None;
                self.tasks[index].capacity_free.push(slot_index);
            }
        } else if number >= REGION_HANDLE_BASE {
            if let Some(slot_index) = number.checked_sub(REGION_HANDLE_BASE + 1)
                && self
                    .process(index)
                    .regions
                    .get(slot_index)
                    .is_some_and(|slot| slot.is_some())
            {
                self.teardown_region(index, slot_index, true);
            }
        } else if number >= CAPABILITY_HANDLE_BASE {
            let Some(slot_index) = number.checked_sub(CAPABILITY_HANDLE_BASE + 1) else {
                return;
            };
            let Some(handle) = self.tasks[index]
                .capabilities
                .get(slot_index)
                .and_then(|slot| slot.as_ref())
                .map(|map| map.handle)
            else {
                return;
            };
            let slots: Vec<usize> = self
                .process(index)
                .regions
                .iter()
                .enumerate()
                .filter_map(|(slot, region)| {
                    region
                        .as_ref()
                        .filter(|region| region.capability == handle)
                        .map(|_| slot)
                })
                .collect();
            for slot in slots {
                self.teardown_region(index, slot, false);
            }
            if let Some(map) = self.tasks[index]
                .capabilities
                .get_mut(slot_index)
                .and_then(|slot| slot.take())
            {
                let _ = self.process_mut(index).ledger.release(map.charge);
                self.tasks[index].capability_free.push(slot_index);
            }
        } else if number >= GRANT_HANDLE_BASE {
            if let Some(slot) = number
                .checked_sub(GRANT_HANDLE_BASE + 1)
                .and_then(|slot| self.tasks[index].grants.get_mut(slot))
                && let Some(entry) = slot.take()
                && let Some(charge) = entry.charge
            {
                let _ = self.process_mut(index).ledger.release(charge);
            }
        } else if number >= EXTRA_HANDLE_BASE {
            if let Some(slot_index) = number.checked_sub(EXTRA_HANDLE_BASE + 1)
                && let Some(slot) = self.tasks[index].extra.get_mut(slot_index)
                && let Some(entry) = slot.take()
            {
                self.tasks[index].extra_free.push(slot_index);
                if let Some(charge) = entry.charge {
                    let _ = self.process_mut(index).ledger.release(charge);
                }
            }
        } else if let Some(slot) = number
            .checked_sub(1)
            .and_then(|slot| self.tasks[index].handles.get_mut(slot))
        {
            *slot = None;
        }
    }

    /// 在任务退出时拆除该进程的全部 Region。
    pub(crate) fn teardown_process(&mut self, index: usize) {
        let slots: Vec<usize> = (0..self.process(index).regions.len()).collect();
        for slot in slots {
            if self
                .process(index)
                .regions
                .get(slot)
                .is_some_and(|region| region.is_some())
            {
                self.teardown_region(index, slot, true);
            }
        }
        self.release_cow_charges(index);
        self.process_mut(index).cow_pages.clear();
        self.release_task_charges(index);
    }

    /// 解除 Region 的映射并关闭它持有的内部句柄。
    ///
    /// `close_capability` 为 true 时同时关闭内核代建的 capability
    /// （`SYS_FRAME_MAP` / 设备映射）；用户自己持有的 capability 不在这里关闭。
    pub(crate) fn teardown_region(&mut self, index: usize, slot: usize, close_capability: bool) {
        let Some(region) = self
            .process(index)
            .regions
            .get(slot)
            .and_then(|region| region.as_ref())
        else {
            return;
        };
        let owner = region.owner;
        let capability = region.capability;
        let internal_capability = region.internal_capability;
        let virt = region.virt;
        let length = region.len;
        let region_id = region.region;
        let region_handle = region.handle;
        let charge = region.charge;
        let pinned = region.flags.contains(hk_mem::hmp::RegionFlags::PINNED);
        let capability_id = self
            .kernel
            .handle_id(owner, capability)
            .ok()
            .map(|id| id.get());
        // 逐页解除，兼容惰性 Region 与已物化 Region；也避免在关闭 capability
        // 之后再去解析它来决定映射范围。
        let page = hk_hal::PAGE_SIZE as u64;
        let mut offset = 0;
        while offset < length as u64 {
            let _ = self
                .platform
                .unmap_user(index, virt + offset, page as usize);
            offset += page;
        }
        self.process_mut(index).user_window.release(virt);
        let _ = self.process_mut(index).hmp_arena.remove(region_id);
        if pinned {
            let _ = self.kernel.pin_capability(owner, capability, false);
        }
        let _ = self.kernel.close_handle(owner, region_handle);
        if close_capability && internal_capability {
            let _ = self.kernel.close_handle(owner, capability);
        }
        self.process_mut(index).regions[slot] = None;
        self.process_mut(index).region_free.push(slot);
        if let Some(charge) = charge {
            let _ = self.process_mut(index).ledger.release(charge);
        }
        if let Some(capability_id) = capability_id {
            self.prune_cow_pages(index, capability_id);
        }
    }

    /// 当能力不再被本进程任何 Region/Arena 引用时，删除它的私有页覆盖。
    pub(crate) fn prune_cow_pages(&mut self, index: usize, capability_id: u64) {
        let referenced = self.process(index).regions.iter().flatten().any(|region| {
            self.kernel
                .handle_id(region.owner, region.capability)
                .ok()
                .map(|id| id.get())
                == Some(capability_id)
        });
        if !referenced {
            let removed: Vec<(u64, usize)> = self
                .process(index)
                .cow_pages
                .keys()
                .filter(|(id, _)| *id == capability_id)
                .copied()
                .collect();
            for key in removed {
                if let Some(page) = self.process_mut(index).cow_pages.remove(&key)
                    && let Some(charge) = page.charge
                {
                    let _ = self.process_mut(index).ledger.release(charge);
                }
            }
        }
    }

    /// 归还全部 COW 私有页记账。
    fn release_cow_charges(&mut self, index: usize) {
        let charges: Vec<Option<Charge>> = self
            .process_mut(index)
            .cow_pages
            .values()
            .map(|page| page.charge)
            .collect();
        for charge in charges.into_iter().flatten() {
            let _ = self.process_mut(index).ledger.release(charge);
        }
        if let Some(charge) = self.process_mut(index).cow_charge.take() {
            let _ = self.process_mut(index).ledger.release(charge);
        }
    }

    /// 清理 fork 后子进程不再引用的覆盖项（例如 DONT_FORK 的能力）。
    pub(crate) fn prune_all_cow_pages(&mut self, index: usize) {
        let ids: Vec<u64> = self
            .process(index)
            .cow_pages
            .keys()
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.prune_cow_pages(index, id);
        }
    }
}
