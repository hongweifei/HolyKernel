//! 句柄复制。

use hk_handle::Rights;

use super::*;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    pub(crate) fn duplicate_handle(&mut self, index: usize, tcb: TcbId, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let Some(handle) = self.handle_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        // Region 句柄不参与复制与转移：它属于创建它的任务，语义由
        // `SYS_REGION_*` 按运行时槽位使用。
        if self.kernel.handle_kind(tcb, handle) == Some(hk_handle::ObjectKind::Region) {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let rights = Rights::from_bits_truncate(frame.arg1() as u32);
        match self.kernel.duplicate_handle(tcb, handle, rights) {
            Ok(copy) => {
                // 复制句柄只增加句柄计数，不重复占用任务的内存配额。
                frame.set_result(self.install_extra_handle(index, copy));
            }
            Err(_) => frame.set_result(u64::MAX),
        }
        Switch::Continue
    }
}
