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
                self.oom.record_event(
                    super::oom::MEMORY_EVENT_RECLAIM,
                    object.get(),
                    page_index as u64 * page,
                    page,
                    owner as u64,
                    0,
                );
                released = released.saturating_add(page);
            }
        }
        released
    }

    /// 回收当前任务中可证明为零的匿名页，返回释放字节数。
    ///
    /// 非零页的压缩/swap 属于 Provider 策略；内核不保存页内容。
    pub fn reclaim_anonymous_pages(&mut self, index: usize, budget: u64) -> u64 {
        self.oom.reclaims = self.oom.reclaims.saturating_add(1);
        let total = self.reclaim_zero_pages(index, budget);
        self.oom.reclaimed = self.oom.reclaimed.saturating_add(total);
        total
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
            self.kernel
                .end_capability_reclaim(tcb, capability, page_index);
            return 0;
        }
        self.kernel
            .end_capability_reclaim(tcb, capability, page_index);
        // SYS_CAPABILITY_SWAP 在发送前已解除全部 PTE，这里只归还物理页。
        let Ok(Some(lease)) = self
            .kernel
            .take_capability_page(tcb, capability, page_index)
        else {
            return 0;
        };
        drop(lease);
        self.release_page_charges(index, object, page_index);
        self.oom.record_event(
            super::oom::MEMORY_EVENT_SWAP,
            object.get(),
            page_index as u64 * PAGE_SIZE as u64,
            PAGE_SIZE as u64,
            index as u64,
            0,
        );
        self.oom.swapped = self.oom.swapped.saturating_add(PAGE_SIZE as u64);
        PAGE_SIZE as u64
    }

    /// 释放被回收页在 Task Ledger 中的记账。
    ///
    /// 同一个能力可能被多个 Region 映射，页只在建立映射的那一段里记过账；
    /// 回收解除了该能力的所有别名，记账也要从所有 Region 中一并移除。
    fn release_page_charges(&mut self, _index: usize, object: ObjectId, page_index: usize) {
        let mut regions: Vec<(usize, usize)> = Vec::new();
        for slot in 0..self.tasks.len() {
            if self.tasks[slot].leader != slot || self.tasks[slot].exited {
                continue;
            }
            for (region, map) in self.process(slot).regions.iter().enumerate() {
                let Some(map) = map.as_ref() else {
                    continue;
                };
                let Ok(id) = self.kernel.capability_id(map.owner, map.capability) else {
                    continue;
                };
                if id == object && map.page_charges.iter().any(|(page, _)| *page == page_index) {
                    regions.push((slot, region));
                }
            }
        }
        for (slot, region) in regions {
            loop {
                let position = self.process(slot).regions[region].as_ref().and_then(|map| {
                    map.page_charges
                        .iter()
                        .position(|(page, _)| *page == page_index)
                });
                let Some(position) = position else {
                    break;
                };
                let Some(map) = self.process_mut(slot).regions[region].as_mut() else {
                    break;
                };
                let (_, charge) = map.page_charges.swap_remove(position);
                let _ = self.process_mut(slot).ledger.release(charge);
            }
        }
    }

    /// 解除该能力第 `page_index` 页在全部进程中的映射。
    pub(crate) fn unmap_capability_page(
        &self,
        _index: usize,
        object: ObjectId,
        page_index: usize,
    ) -> bool {
        let page = PAGE_SIZE as u64;
        let capability_offset = page_index as u64 * page;
        let mut unmapped = false;
        for slot in 0..self.tasks.len() {
            if self.tasks[slot].leader != slot || self.tasks[slot].exited {
                continue;
            }
            for region in self.process(slot).regions.iter().flatten() {
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
                if self.platform.unmap_user(slot, virt, page as usize) {
                    unmapped = true;
                }
            }
        }
        unmapped
    }
}
