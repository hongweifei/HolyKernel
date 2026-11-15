//! 句柄复制。

use hk_handle::Rights;

use super::*;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    /// 返回句柄对应的对象标识；Provider 用它关联缺页消息中的能力。
    pub(crate) fn object_id(&mut self, index: usize, frame: &mut C) -> Switch {
        let number = frame.arg0() as usize;
        let Some(handle) = self.handle_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        match self.kernel.handle_id(self.tasks[index].tcb, handle) {
            Ok(id) => frame.set_result(id.get()),
            Err(_) => frame.set_result(u64::MAX),
        }
        Switch::Continue
    }

    /// 把最近一次接收消息的 8 个字复制到用户缓冲。
    pub(crate) fn message_words(&mut self, index: usize, frame: &mut C) -> Switch {
        let out = frame.arg0();
        let mut bytes = [0_u8; hk_ipc::MAX_MESSAGE_WORDS * 8];
        for (slot, word) in self.tasks[index].last_words.iter().enumerate() {
            bytes[slot * 8..slot * 8 + 8].copy_from_slice(&word.to_le_bytes());
        }
        frame.set_result(if out != 0 && self.platform.write_user(out, &bytes) {
            bytes.len() as u64
        } else {
            u64::MAX
        });
        Switch::Continue
    }
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
