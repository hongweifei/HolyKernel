//! 用户页的 NUMA 迁移：保留内容，把 Frame 换到目标节点。

use alloc::sync::Arc;

use hk_mem::hmp::{Capacity, FrameBacking, OwnedFrame};

use crate::runtime::{UserPlatform, UserRuntime};
use hk_hal::{SyscallFrame, UserContext};
use hk_handle::Handle;
use hk_kernel::TcbId;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    /// 把能力的一段常驻页迁移到 `node`，返回迁移字节数。
    ///
    /// 迁移保留任务 Ledger 记账不变；PTE 先解除，下一次缺页按新物理页重建。
    /// 共享页与导入区间跳过，避免破坏其他地址空间或设备所有权。
    pub fn migrate_capability(
        &mut self,
        index: usize,
        tcb: TcbId,
        handle: Handle,
        offset: u64,
        len: u64,
        node: u8,
    ) -> u64 {
        let page = hk_hal::PAGE_SIZE as u64;
        let Ok((capability, backing)) = self.kernel.capability(tcb, handle) else {
            return 0;
        };
        if backing.is_some() || capability.size() == 0 {
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
        let mut migrated = 0_u64;
        let mut page_index = (start / page) as usize;
        let last = (end / page) as usize;
        while page_index < last {
            let Ok(Some(old)) = self.kernel.capability_page(tcb, handle, page_index) else {
                page_index += 1;
                continue;
            };
            if self.platform.page_shared(old.start()) {
                page_index += 1;
                continue;
            }
            if self.platform.page_node(old.start()) == Some(node) {
                page_index += 1;
                continue;
            }
            let Ok(Some(backing)) = self.kernel.take_capability_page(tcb, handle, page_index)
            else {
                page_index += 1;
                continue;
            };
            let Some(capacity): Option<&Arc<spin::Mutex<Capacity>>> = backing.capacity() else {
                let _ = self
                    .kernel
                    .set_capability_page(tcb, handle, page_index, backing);
                page_index += 1;
                continue;
            };
            let capacity = Arc::clone(capacity);
            let Some(frame) = capacity
                .lock()
                .allocate_on(hk_hal::PAGE_SIZE, Some(node))
                .ok()
            else {
                let _ = self
                    .kernel
                    .set_capability_page(tcb, handle, page_index, backing);
                page_index += 1;
                continue;
            };
            let new = hk_hal::PhysRange::new(frame.phys(), frame.len() as u64);
            let mut data = [0_u8; hk_hal::PAGE_SIZE];
            if !self.platform.read_phys(old.start(), &mut data)
                || !self.platform.write_phys(new.start(), &data)
            {
                let _ = self
                    .kernel
                    .set_capability_page(tcb, handle, page_index, backing);
                page_index += 1;
                continue;
            }
            let new_backing = FrameBacking::Owned(OwnedFrame::new(frame, capacity));
            if !matches!(
                self.kernel
                    .set_capability_page(tcb, handle, page_index, new_backing),
                Ok(true)
            ) {
                let _ = self
                    .kernel
                    .set_capability_page(tcb, handle, page_index, backing);
                page_index += 1;
                continue;
            }
            // PTE 需要在页表层面重建；先解除，下一次访问走常驻页快速路径。
            if !self.unmap_capability_page(index, object, page_index) {
                // 没有解除映射时恢复旧帧，避免 PTE 指向已归还的物理页。
                if let Ok(Some(new)) = self.kernel.take_capability_page(tcb, handle, page_index) {
                    let _ = self
                        .kernel
                        .set_capability_page(tcb, handle, page_index, backing);
                    drop(new);
                }
                page_index += 1;
                continue;
            }
            drop(backing);
            self.oom.record_event(
                super::oom::MEMORY_EVENT_MIGRATE,
                object.get(),
                page_index as u64 * page,
                page,
                index as u64,
                u64::from(node),
            );
            self.notify_memory_event(super::oom::MEMORY_EVENT_MIGRATE);
            migrated = migrated.saturating_add(page);
            page_index += 1;
        }
        self.oom.migrated = self.oom.migrated.saturating_add(migrated);
        migrated
    }
}
