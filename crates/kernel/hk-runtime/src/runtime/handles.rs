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
        let Ok(charge) = self.tasks[index].ledger.charge(capability.size()) else {
            let _ = self.kernel.close_handle(tcb, handle);
            return None;
        };
        let number = self.install_extra_handle(index, handle);
        let slot = number as usize - EXTRA_HANDLE_BASE - 1;
        self.tasks[index].extra_charges[slot] = Some(charge);
        Some(number)
    }

    /// 复制父任务的 capability/Region 元数据到 fork 子任务。
    pub(crate) fn fork_regions(&mut self, parent: usize, child: usize) {
        let entries: Vec<(Handle, Handle, u64, usize)> = self.tasks[parent]
            .regions
            .iter()
            .flatten()
            .map(|region| (region.handle, region.capability, region.virt, region.len))
            .collect();
        let parent_tcb = self.tasks[parent].tcb;
        let child_tcb = self.tasks[child].tcb;
        let rights = Rights::READ | Rights::WRITE | Rights::TRANSFER | Rights::ADMIN;
        for (region_handle, capability_handle, virt, len) in entries {
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
            let region_id = region.id();
            if self.tasks[child]
                .hmp_arena
                .as_mut()
                .is_some_and(|arena| arena.insert(&region).is_err())
            {
                let _ = self.kernel.close_handle(child_tcb, capability);
                continue;
            }
            let Ok(child_region) = self.kernel.create_region(child_tcb, region, rights) else {
                let _ = self.kernel.close_handle(child_tcb, capability);
                if let Some(arena) = self.tasks[child].hmp_arena.as_mut() {
                    let _ = arena.remove(region_id);
                }
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
            let Ok(charge) = self.tasks[child].ledger.charge(size) else {
                let _ = self.kernel.close_handle(child_tcb, capability);
                let _ = self.kernel.close_handle(child_tcb, child_region);
                continue;
            };
            self.install_capability(child, capability, charge);
            self.install_region(child, child_region, capability, region_id, virt, len);
        }
    }

    pub(crate) fn install_extra_handle(&mut self, index: usize, handle: Handle) -> u64 {
        let slot = self.tasks[index].extra.len();
        self.tasks[index].extra.push(Some(handle));
        self.tasks[index].extra_charges.push(None);
        (EXTRA_HANDLE_BASE + slot + 1) as u64
    }

    pub(crate) fn install_capability(
        &mut self,
        index: usize,
        handle: Handle,
        charge: Charge,
    ) -> u64 {
        let slot = self.tasks[index]
            .capabilities
            .iter()
            .position(Option::is_none)
            .unwrap_or(self.tasks[index].capabilities.len());
        let map = CapabilityMap { handle, charge };
        if slot == self.tasks[index].capabilities.len() {
            self.tasks[index].capabilities.push(Some(map));
        } else {
            self.tasks[index].capabilities[slot] = Some(map);
        }
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
            *self.tasks[index]
                .extra
                .get(number.checked_sub(EXTRA_HANDLE_BASE + 1)?)?
                .as_ref()?
        };
        let (capability, backing) = self.kernel.capability(self.tasks[index].tcb, handle).ok()?;
        Some((handle, capability, backing))
    }

    pub(crate) fn install_region(
        &mut self,
        index: usize,
        handle: Handle,
        capability: Handle,
        region: hk_mem::hmp::RegionId,
        virt: u64,
        len: usize,
    ) -> u64 {
        let slot = self.tasks[index]
            .regions
            .iter()
            .position(Option::is_none)
            .unwrap_or(self.tasks[index].regions.len());
        let map = RegionMap {
            handle,
            capability,
            region,
            virt,
            len,
        };
        if slot == self.tasks[index].regions.len() {
            self.tasks[index].regions.push(Some(map));
        } else {
            self.tasks[index].regions[slot] = Some(map);
        }
        (REGION_HANDLE_BASE + slot + 1) as u64
    }

    pub(crate) fn region_at(
        &self,
        index: usize,
        number: usize,
    ) -> Option<(usize, Handle, u64, usize)> {
        let slot = number.checked_sub(REGION_HANDLE_BASE + 1)?;
        let map = self.tasks[index].regions.get(slot)?.as_ref()?;
        Some((slot, map.handle, map.virt, map.len))
    }

    pub(crate) fn install_capacity(&mut self, index: usize, handle: Handle) -> u64 {
        let slot = self.tasks[index]
            .capacities
            .iter()
            .position(Option::is_none)
            .unwrap_or(self.tasks[index].capacities.len());
        if slot == self.tasks[index].capacities.len() {
            self.tasks[index].capacities.push(Some(handle));
        } else {
            self.tasks[index].capacities[slot] = Some(handle);
        }
        (CAPACITY_HANDLE_BASE + slot + 1) as u64
    }

    pub(crate) fn capacity_at(&self, index: usize, number: usize) -> Option<(usize, Handle)> {
        let slot = number.checked_sub(CAPACITY_HANDLE_BASE + 1)?;
        let handle = self.tasks[index].capacities.get(slot)?.as_ref()?;
        Some((slot, *handle))
    }

    pub(crate) fn install_frame(&mut self, index: usize, handle: Handle) -> u64 {
        let slot = self.tasks[index]
            .frames
            .iter()
            .position(Option::is_none)
            .unwrap_or(self.tasks[index].frames.len());
        if slot == self.tasks[index].frames.len() {
            self.tasks[index].frames.push(Some(handle));
        } else {
            self.tasks[index].frames[slot] = Some(handle);
        }
        (FRAME_HANDLE_BASE + slot + 1) as u64
    }

    pub(crate) fn frame_at(&self, index: usize, number: usize) -> Option<(usize, Handle)> {
        let slot = number.checked_sub(FRAME_HANDLE_BASE + 1)?;
        let handle = self.tasks[index].frames.get(slot)?.as_ref()?;
        Some((slot, *handle))
    }

    pub(crate) fn grant_at(&self, index: usize, number: usize) -> Option<Handle> {
        if let Some(handle) = number
            .checked_sub(GRANT_HANDLE_BASE + 1)
            .and_then(|slot| self.tasks[index].grants.get(slot))
            .copied()
            .flatten()
        {
            return Some(handle);
        }
        let handle = self.tasks[index]
            .extra
            .get(number.checked_sub(EXTRA_HANDLE_BASE + 1)?)
            .copied()
            .flatten()?;
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
        if slot == self.tasks[index].grants.len() {
            self.tasks[index].grants.push(Some(handle));
            self.tasks[index].grant_charges.push(charge);
        } else {
            self.tasks[index].grants[slot] = Some(handle);
            self.tasks[index].grant_charges[slot] = charge;
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
                .copied()
                .flatten()
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
            if let Some(slot) = number
                .checked_sub(FRAME_HANDLE_BASE + 1)
                .and_then(|slot| self.tasks[index].frames.get_mut(slot))
            {
                *slot = None;
            }
        } else if number >= CAPACITY_HANDLE_BASE {
            if let Some(slot) = number
                .checked_sub(CAPACITY_HANDLE_BASE + 1)
                .and_then(|slot| self.tasks[index].capacities.get_mut(slot))
            {
                *slot = None;
            }
        } else if number >= REGION_HANDLE_BASE {
            if let Some(slot) = number
                .checked_sub(REGION_HANDLE_BASE + 1)
                .and_then(|slot| self.tasks[index].regions.get_mut(slot))
                && let Some(map) = slot.take()
                && let Some(arena) = self.tasks[index].hmp_arena.as_mut()
            {
                let _ = arena.remove(map.region);
            }
        } else if number >= CAPABILITY_HANDLE_BASE {
            if let Some(slot) = number
                .checked_sub(CAPABILITY_HANDLE_BASE + 1)
                .and_then(|slot| self.tasks[index].capabilities.get_mut(slot))
                && let Some(map) = slot.take()
            {
                let _ = self.tasks[index].ledger.release(map.charge);
            }
        } else if number >= GRANT_HANDLE_BASE {
            let slot = number.checked_sub(GRANT_HANDLE_BASE + 1);
            if let Some(slot) = slot
                && slot < self.tasks[index].grants.len()
            {
                self.tasks[index].grants[slot] = None;
                if let Some(charge) = self.tasks[index].grant_charges[slot].take() {
                    let _ = self.tasks[index].ledger.release(charge);
                }
            }
        } else if number >= EXTRA_HANDLE_BASE {
            let slot = number - EXTRA_HANDLE_BASE - 1;
            if let Some(handle) = self.tasks[index].extra.get_mut(slot) {
                *handle = None;
                if let Some(charge) = self.tasks[index]
                    .extra_charges
                    .get_mut(slot)
                    .and_then(Option::take)
                {
                    let _ = self.tasks[index].ledger.release(charge);
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
