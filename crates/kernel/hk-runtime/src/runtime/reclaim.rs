//! 池耗尽时的内存回收：把可证明为零的匿名页还给池。
//!
//! 回收只处理匿名能力中内容全为零的页：这类页的内容与重新缺页时内核
//! 提供的零页完全一致，回收不改变语义；绑定 Provider 的匿名能力要求
//! Provider 对被回收的页同样提供零页。常驻能力、固定驻留（含 scanout）
//! 与已写过的页都不在回收范围内。

use alloc::vec::Vec;

use hk_hal::{PAGE_SIZE, SyscallFrame, UserContext};
use hk_handle::{Handle, ObjectId};
use hk_kernel::TcbId;
use hk_mem::hmp::{CapabilityKind, RegionFlags};

use super::{UserPlatform, UserRuntime};

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    /// 回收当前任务中可证明为零的匿名页，返回释放字节数。
    pub fn reclaim_zero_pages(&mut self, index: usize, budget: u64) -> u64 {
        let owner = self.leader_of(index);
        let page = PAGE_SIZE as u64;
        let regions: Vec<(TcbId, Handle)> = self
            .process(owner)
            .regions
            .iter()
            .flatten()
            .map(|region| (region.owner, region.capability))
            .collect();
        // 只要同一个 capability 有任一 PINNED 别名，就不能回收其页，
        // 否则另一个固定驻留映射会看到悬空 PTE。
        let pinned: Vec<ObjectId> = self
            .process(owner)
            .regions
            .iter()
            .flatten()
            .filter(|region| region.flags.contains(RegionFlags::PINNED))
            .filter_map(|region| {
                self.kernel
                    .capability_id(region.owner, region.capability)
                    .ok()
            })
            .collect();
        let mut released = 0;
        for (tcb, handle) in regions {
            if released >= budget {
                break;
            }
            let Ok((capability, backing)) = self.kernel.capability(tcb, handle) else {
                continue;
            };
            if backing.is_some()
                || capability.kind() != CapabilityKind::Anonymous
                || capability.size() == 0
            {
                continue;
            }
            if !matches!(self.kernel.capability_pinned(tcb, handle), Ok(false)) {
                continue;
            }
            let Ok(object) = self.kernel.capability_id(tcb, handle) else {
                continue;
            };
            if pinned.contains(&object) {
                continue;
            }
            let provider = self.kernel.capability_provider(tcb, handle).ok().flatten();
            let pages = (capability.size() / page) as usize;
            for page_index in 0..pages {
                if released >= budget {
                    break;
                }
                let Ok(Some(range)) = self.kernel.capability_page(tcb, handle, page_index) else {
                    continue;
                };
                // 共享页仍被其他地址空间映射，回收会留下悬空 PTE。
                if self.platform.page_shared(range.start()) {
                    continue;
                }
                if self.platform.page_is_zero(range.start()) != Some(true) {
                    continue;
                }
                if !self.unmap_capability_page(index, object, page_index) {
                    continue;
                }
                let Ok(Some(lease)) = self.kernel.take_capability_page(tcb, handle, page_index)
                else {
                    continue;
                };
                drop(lease);
                if let Some(provider) = provider {
                    let mut words = [0_u64; 8];
                    words[0] = object.get();
                    words[1] = page_index as u64 * page;
                    words[2] = page;
                    let _ = self.kernel.post_to_endpoint(
                        provider,
                        hk_ipc::Message::new(hk_syscall::PROVIDER_EVICT).with_words(words),
                    );
                }
                self.release_page_charges(index, object, page_index);
                released = released.saturating_add(page);
            }
        }
        released
    }

    /// 回收当前任务中可证明为零或可压缩的匿名页，返回释放字节数。
    ///
    /// 零页直接归还；非零页先压缩，只有压缩块明显小于一页时才换出，否则保留
    /// 原页。压缩块保存在能力对象中，缺页时由内核解压重装。
    pub fn reclaim_anonymous_pages(&mut self, index: usize, budget: u64) -> u64 {
        self.oom.reclaims = self.oom.reclaims.saturating_add(1);
        let zero = self.reclaim_zero_pages(index, budget);
        let total = if zero >= budget {
            zero
        } else {
            zero.saturating_add(self.reclaim_compressed_pages(index, budget - zero))
        };
        self.oom.reclaimed = self.oom.reclaimed.saturating_add(total);
        total
    }

    /// 压缩换出一个匿名能力的指定范围，返回释放字节数。
    ///
    /// 供 `SYS_CAPABILITY_SWAP` 使用：调用方明确要求降内存，不等池耗尽。
    pub(crate) fn swap_capability_pages(
        &mut self,
        index: usize,
        tcb: TcbId,
        handle: Handle,
        offset: u64,
        len: u64,
    ) -> u64 {
        let page = PAGE_SIZE as u64;
        let Ok((capability, backing)) = self.kernel.capability(tcb, handle) else {
            return 0;
        };
        if backing.is_some()
            || capability.kind() != CapabilityKind::Anonymous
            || capability.size() == 0
        {
            return 0;
        }
        if !matches!(self.kernel.capability_pinned(tcb, handle), Ok(false)) {
            return 0;
        }
        let Ok(object) = self.kernel.capability_id(tcb, handle) else {
            return 0;
        };
        let start = offset / page * page;
        let end = start
            .saturating_add(len.max(page))
            .min(capability.size())
            .div_ceil(page)
            * page;
        let provider = self.kernel.capability_provider(tcb, handle).ok().flatten();
        let mut released = 0_u64;
        let mut page_index = (start / page) as usize;
        let last = (end / page) as usize;
        while page_index < last {
            let Ok(Some(range)) = self.kernel.capability_page(tcb, handle, page_index) else {
                page_index += 1;
                continue;
            };
            if self.platform.page_shared(range.start()) {
                page_index += 1;
                continue;
            }
            let mut data = [0_u8; PAGE_SIZE];
            let Some(compressed) = self
                .platform
                .read_phys(range.start(), &mut data)
                .then(|| super::swap::compress(&data))
                .flatten()
            else {
                page_index += 1;
                continue;
            };
            let Ok(Some(lease)) = self.kernel.take_capability_page(tcb, handle, page_index) else {
                page_index += 1;
                continue;
            };
            if !matches!(
                self.kernel
                    .set_capability_swapped(tcb, handle, page_index, compressed),
                Ok(true)
            ) {
                let _ = self
                    .kernel
                    .set_capability_page(tcb, handle, page_index, lease);
                page_index += 1;
                continue;
            }
            if !self.unmap_capability_page(index, object, page_index) {
                let _ = self.kernel.take_capability_swapped(tcb, handle, page_index);
                let _ = self
                    .kernel
                    .set_capability_page(tcb, handle, page_index, lease);
                page_index += 1;
                continue;
            }
            drop(lease);
            self.release_page_charges(index, object, page_index);
            if let Some(provider) = provider {
                let mut words = [0_u64; 8];
                words[0] = object.get();
                words[1] = page_index as u64 * page;
                words[2] = page;
                let _ = self.kernel.post_to_endpoint(
                    provider,
                    hk_ipc::Message::new(hk_syscall::PROVIDER_EVICT).with_words(words),
                );
            }
            released = released.saturating_add(page);
            page_index += 1;
        }
        self.oom.swapped = self.oom.swapped.saturating_add(released);
        released
    }

    /// 压缩并换出非零匿名页；共享、固定驻留与已映射到其他地址空间的页跳过。
    fn reclaim_compressed_pages(&mut self, index: usize, budget: u64) -> u64 {
        let owner = self.leader_of(index);
        let page = PAGE_SIZE as u64;
        let regions: Vec<(TcbId, Handle)> = self
            .process(owner)
            .regions
            .iter()
            .flatten()
            .map(|region| (region.owner, region.capability))
            .collect();
        let pinned: Vec<ObjectId> = self
            .process(owner)
            .regions
            .iter()
            .flatten()
            .filter(|region| region.flags.contains(RegionFlags::PINNED))
            .filter_map(|region| {
                self.kernel
                    .capability_id(region.owner, region.capability)
                    .ok()
            })
            .collect();
        let mut released = 0;
        for (tcb, handle) in regions {
            if released >= budget {
                break;
            }
            let Ok((capability, backing)) = self.kernel.capability(tcb, handle) else {
                continue;
            };
            if backing.is_some()
                || capability.kind() != CapabilityKind::Anonymous
                || capability.size() == 0
            {
                continue;
            }
            if !matches!(self.kernel.capability_pinned(tcb, handle), Ok(false)) {
                continue;
            }
            let Ok(object) = self.kernel.capability_id(tcb, handle) else {
                continue;
            };
            if pinned.contains(&object) {
                continue;
            }
            let provider = self.kernel.capability_provider(tcb, handle).ok().flatten();
            let pages = (capability.size() / page) as usize;
            for page_index in 0..pages {
                if released >= budget {
                    break;
                }
                let Ok(Some(range)) = self.kernel.capability_page(tcb, handle, page_index) else {
                    continue;
                };
                if self.platform.page_shared(range.start()) {
                    continue;
                }
                if self.platform.page_is_zero(range.start()) == Some(true) {
                    continue;
                }
                let mut data = [0_u8; PAGE_SIZE];
                if !self.platform.read_phys(range.start(), &mut data) {
                    continue;
                }
                let Some(compressed) = super::swap::compress(&data) else {
                    continue;
                };
                let Ok(Some(lease)) = self.kernel.take_capability_page(tcb, handle, page_index)
                else {
                    continue;
                };
                if !matches!(
                    self.kernel
                        .set_capability_swapped(tcb, handle, page_index, compressed),
                    Ok(true)
                ) {
                    let _ = self
                        .kernel
                        .set_capability_page(tcb, handle, page_index, lease);
                    continue;
                }
                if !self.unmap_capability_page(index, object, page_index) {
                    // 没有成功解除映射时恢复原页，避免 PTE 指向已经归还的帧。
                    let _ = self.kernel.take_capability_swapped(tcb, handle, page_index);
                    let _ = self
                        .kernel
                        .set_capability_page(tcb, handle, page_index, lease);
                    continue;
                }
                drop(lease);
                self.release_page_charges(index, object, page_index);
                if let Some(provider) = provider {
                    let mut words = [0_u64; 8];
                    words[0] = object.get();
                    words[1] = page_index as u64 * page;
                    words[2] = page;
                    let _ = self.kernel.post_to_endpoint(
                        provider,
                        hk_ipc::Message::new(hk_syscall::PROVIDER_EVICT).with_words(words),
                    );
                }
                released = released.saturating_add(page);
            }
        }
        self.oom.swapped = self.oom.swapped.saturating_add(released);
        released
    }

    /// Provider 确认内容已保存后拆除映射、归还物理页并释放 Ledger。
    pub(crate) fn finish_provider_reclaim(
        &mut self,
        index: usize,
        capability: Handle,
        object: ObjectId,
        page_index: usize,
    ) -> u64 {
        let owner = self.leader_of(index);
        let tcb = self.tasks[owner].tcb;
        let Ok(id) = self.kernel.capability_id(tcb, capability) else {
            return 0;
        };
        if id != object {
            return 0;
        }
        if !self.unmap_capability_page(index, object, page_index) {
            return 0;
        }
        let Ok(Some(lease)) = self
            .kernel
            .take_capability_page(tcb, capability, page_index)
        else {
            return 0;
        };
        drop(lease);
        self.release_page_charges(index, object, page_index);
        self.oom.swapped = self.oom.swapped.saturating_add(PAGE_SIZE as u64);
        PAGE_SIZE as u64
    }

    /// 释放被回收页在 Task Ledger 中的记账。
    ///
    /// 同一个能力可能被多个 Region 映射，页只在建立映射的那一段里记过账；
    /// 回收解除了该能力的所有别名，记账也要从所有 Region 中一并移除。
    fn release_page_charges(&mut self, index: usize, object: ObjectId, page_index: usize) {
        let slots: Vec<usize> = self
            .process(index)
            .regions
            .iter()
            .enumerate()
            .filter_map(|(slot, map)| {
                let map = map.as_ref()?;
                let id = self.kernel.capability_id(map.owner, map.capability).ok()?;
                (id == object && map.page_charges.iter().any(|(page, _)| *page == page_index))
                    .then_some(slot)
            })
            .collect();
        for slot in slots {
            loop {
                let position = self
                    .process(index)
                    .regions
                    .get(slot)
                    .and_then(|map| map.as_ref())
                    .and_then(|map| {
                        map.page_charges
                            .iter()
                            .position(|(page, _)| *page == page_index)
                    });
                let Some(position) = position else {
                    break;
                };
                let Some(map) = self.process_mut(index).regions[slot].as_mut() else {
                    break;
                };
                let (_, charge) = map.page_charges.swap_remove(position);
                let _ = self.process_mut(index).ledger.release(charge);
            }
        }
    }

    /// 解除映射该能力第 `page_index` 页的所有映射。
    pub(crate) fn unmap_capability_page(
        &self,
        index: usize,
        object: ObjectId,
        page_index: usize,
    ) -> bool {
        let page = PAGE_SIZE as u64;
        let capability_offset = page_index as u64 * page;
        let mut unmapped = false;
        for region in self.process(index).regions.iter().flatten() {
            let Ok(id) = self.kernel.capability_id(region.owner, region.capability) else {
                continue;
            };
            if id != object {
                continue;
            }
            let start = region.offset;
            let end = start.saturating_add(region.len as u64);
            if capability_offset < start || capability_offset + page > end {
                continue;
            }
            let virt = region.virt + (capability_offset - start);
            if self.platform.unmap_user(index, virt, page as usize) {
                unmapped = true;
            }
        }
        unmapped
    }
}
