//! System Capacity 记账快照。

use super::*;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    pub(crate) fn memory_stats(&mut self, frame: &mut C) -> Switch {
        let out = frame.arg0();
        let capacity = frame.arg1() as usize;
        if out == 0 || capacity < 88 {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let Some(stats) = self.platform.memory_stats() else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let mut bytes = [0_u8; 88];
        bytes[..8].copy_from_slice(&stats.total.to_le_bytes());
        bytes[8..16].copy_from_slice(&stats.used.to_le_bytes());
        bytes[16..24].copy_from_slice(&stats.peak.to_le_bytes());
        bytes[24..32].copy_from_slice(&stats.free.to_le_bytes());
        for (index, value) in stats.by_kind.iter().enumerate() {
            let start = 32 + index * 8;
            bytes[start..start + 8].copy_from_slice(&value.to_le_bytes());
        }
        frame.set_result(if self.platform.write_user(out, &bytes) {
            88
        } else {
            u64::MAX
        });
        Switch::Continue
    }
}
