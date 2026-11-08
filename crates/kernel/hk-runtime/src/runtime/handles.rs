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
            return Some(self.install_extra_handle(index, handle));
        };
        let Ok(charge) = self.process_mut(index).ledger.charge(capability.size()) else {
            let _ = self.kernel.close_handle(tcb, handle);
            return None;
        };
        Some(self.install_extra_entry(index, handle, Some(charge)))
    }

    /// 复制父任务的 capability/Region 元数据到 fork 子任务。
    pub(crate) fn fork_regions(&mut self, parent: usize, child: usize) {
        let entries: Vec<(Handle, Handle, u64, usize, u64)> = self
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
                )
            })
            .collect();
        let parent_tcb = self.tasks[parent].tcb;
        let child_tcb = self.tasks[child].tcb;
        let rights = Rights::READ | Rights::WRITE | Rights::TRANSFER | Rights::ADMIN;
        for (region_handle, capability_handle, virt, len, offset) in entries {
            let Ok(capability) =
                self.kernel
                    .duplicate_handle(parent_tcb, capability_handle, rights)
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
            let Ok(child_region) = self.kernel.create_region(child_tcb, region, rights) else {
                let _ = self.kernel.close_handle(child_tcb, capability);
                let _ = self.process_mut(child).hmp_arena.remove(region_id);
                continue;
            };
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
                None,
                virt,
                len,
                offset,
                region_rights,
                region_flags,
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
        frame: Option<Handle>,
        virt: u64,
        len: usize,
        offset: u64,
        rights: Rights,
        flags: hk_mem::hmp::RegionFlags,
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
            frame,
            virt,
            len,
            offset,
            rights,
            flags,
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

    pub(crate) fn install_frame(&mut self, index: usize, handle: Handle) -> u64 {
        let slot = match self.tasks[index].frame_free.pop() {
            Some(slot) => slot,
            None => {
                self.tasks[index].frames.push(None);
                self.tasks[index].frames.len() - 1
            }
        };
        self.tasks[index].frames[slot] = Some(handle);
        (FRAME_HANDLE_BASE + slot + 1) as u64
    }

    pub(crate) fn frame_at(&self, index: usize, number: usize) -> Option<(usize, Handle)> {
        let slot = number.checked_sub(FRAME_HANDLE_BASE + 1)?;
        let handle = self.tasks[index].frames.get(slot)?.as_ref()?;
        Some((slot, *handle))
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

    pub(crate) fn clear_handle(&mut self, index: usize, number: usize) {
        if number >= FRAME_HANDLE_BASE {
            if let Some(slot_index) = number.checked_sub(FRAME_HANDLE_BASE + 1)
                && let Some(slot) = self.tasks[index].frames.get_mut(slot_index)
                && slot.is_some()
            {
                *slot = None;
                self.tasks[index].frame_free.push(slot_index);
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
                && let Some(slot) = self.process_mut(index).regions.get_mut(slot_index)
                && let Some(map) = slot.take()
            {
                let _ = self.process_mut(index).hmp_arena.remove(map.region);
                self.process_mut(index).region_free.push(slot_index);
            }
        } else if number >= CAPABILITY_HANDLE_BASE {
            if let Some(slot_index) = number.checked_sub(CAPABILITY_HANDLE_BASE + 1)
                && let Some(slot) = self.tasks[index].capabilities.get_mut(slot_index)
                && let Some(map) = slot.take()
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
}
