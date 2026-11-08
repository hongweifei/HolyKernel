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
use hk_mem::hmp::CapabilityKind;

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
                if !self.unmap_capability_page(index, object, page_index) {
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
    fn unmap_capability_page(&self, index: usize, object: ObjectId, page_index: usize) -> bool {
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
