//! 任务 Arena 查询。

use super::*;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    pub(crate) fn arena_info(&mut self, index: usize, frame: &mut C) -> Switch {
        let out = frame.arg0();
        let capacity = frame.arg1() as usize;
        if out == 0 || capacity < 32 {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let Some(arena) = self.tasks[index].hmp_arena.as_ref() else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let mut bytes = [0_u8; 32];
        bytes[..8].copy_from_slice(&arena.base().get().to_le_bytes());
        bytes[8..16].copy_from_slice(&arena.size().to_le_bytes());
        bytes[16..24].copy_from_slice(&arena.used().to_le_bytes());
        bytes[24..].copy_from_slice(&(arena.regions().len() as u64).to_le_bytes());
        frame.set_result(if self.platform.write_user(out, &bytes) {
            32
        } else {
            u64::MAX
        });
        Switch::Continue
    }
}
