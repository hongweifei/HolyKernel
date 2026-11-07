//! 池耗尽时的内存回收：把可证明为零的匿名页还给池。
//!
//! 回收只处理匿名能力中内容全为零的页：这类页的内容与重新缺页时内核
//! 提供的零页完全一致，回收不改变语义；绑定 Provider 的匿名能力要求
//! Provider 对被回收的页同样提供零页。常驻能力、固定驻留（含 scanout）
//! 与已写过的页都不在回收范围内。

use alloc::vec::Vec;

use hk_hal::{PAGE_SIZE, SyscallFrame, UserContext};
use hk_handle::{Handle, ObjectId};
use hk_mem::hmp::CapabilityKind;

use super::{UserPlatform, UserRuntime};

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    /// 分配一段 DMA 窗口；池耗尽时先回收当前任务的零页再重试一次。
    pub(crate) fn allocate_window(&mut self, index: usize, len: u64) -> Option<hk_hal::PhysRange> {
        if let Some(range) = self.platform.dma_window(len as usize) {
            return Some(range);
        }
        if self.reclaim_zero_pages(index, len) == 0 {
            return None;
        }
        self.platform.dma_window(len as usize)
    }

    /// 回收当前任务中可证明为零的匿名页，返回释放字节数。
    pub fn reclaim_zero_pages(&mut self, index: usize, budget: u64) -> u64 {
        let owner = self.leader_of(index);
        let tcb = self.tasks[owner].tcb;
        let page = PAGE_SIZE as u64;
        let regions: Vec<Handle> = self.tasks[owner]
            .regions
            .iter()
            .flatten()
            .map(|region| region.capability)
            .collect();
        let mut released = 0;
        for handle in regions {
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
            let pages = (capability.size() / page) as usize;
            for page_index in 0..pages {
                if released >= budget {
                    break;
                }
                let Ok(Some(range)) = self.kernel.capability_page(tcb, handle, page_index) else {
                    continue;
                };
                if self.platform.page_is_zero(range.start()) != Some(true) {
                    continue;
                }
                if !self.unmap_capability_page(owner, index, object, page_index) {
                    continue;
                }
                let Ok(Some(lease)) = self.kernel.take_capability_page(tcb, handle, page_index)
                else {
                    continue;
                };
                drop(lease);
                released = released.saturating_add(page);
            }
        }
        released
    }

    /// 解除映射该能力第 `page_index` 页的所有映射。
    fn unmap_capability_page(
        &self,
        owner: usize,
        index: usize,
        object: ObjectId,
        page_index: usize,
    ) -> bool {
        let tcb = self.tasks[owner].tcb;
        let page = PAGE_SIZE as u64;
        let offset = page_index as u64 * page;
        let mut unmapped = false;
        for region in self.tasks[owner].regions.iter().flatten() {
            let Ok(id) = self.kernel.capability_id(tcb, region.capability) else {
                continue;
            };
            if id != object || offset + page > region.len as u64 {
                continue;
            }
            if self
                .platform
                .unmap_dma(index, region.virt + offset, page as usize)
            {
                unmapped = true;
            }
        }
        unmapped
    }
}
