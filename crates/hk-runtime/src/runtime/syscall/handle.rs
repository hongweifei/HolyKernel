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
        let rights = Rights::from_bits_truncate(frame.arg1() as u32);
        match self.kernel.duplicate_handle(tcb, handle, rights) {
            Ok(copy) => {
                let number = self.install_extra_handle(index, copy);
                frame.set_result(number);
            }
            Err(_) => frame.set_result(u64::MAX),
        }
        Switch::Continue
    }
}
