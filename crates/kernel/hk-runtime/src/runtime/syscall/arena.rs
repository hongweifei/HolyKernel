//! 任务 Arena 查询。

use super::*;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    /// 查询任务 Arena 的地址窗口与 Region 统计。
    ///
    /// `size` 是 Arena 覆盖的地址窗口，`used` 是已登记 Region 的字节数；
    /// `SYS_ALLOC` 的可分配窗口与游标由 `TaskInfo.arena_size`/`arena_used`
    /// 报告。两者口径不同：Arena 窗口包含镜像、栈与动态窗口映射。
    pub(crate) fn arena_info(&mut self, index: usize, frame: &mut C) -> Switch {
        let out = frame.arg0();
        let capacity = frame.arg1() as usize;
        if out == 0 || capacity < 32 {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let arena = &self.process(index).hmp_arena;
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

    /// 查询 System Arena 快照；编码与 `SYS_ARENA_INFO` 相同。
    pub(crate) fn system_arena_info(&mut self, frame: &mut C) -> Switch {
        let out = frame.arg0();
        let capacity = frame.arg1() as usize;
        if out == 0 || capacity < 32 {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let Some(info) = self.platform.system_arena_info() else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let mut bytes = [0_u8; 32];
        bytes[..8].copy_from_slice(&info.base.to_le_bytes());
        bytes[8..16].copy_from_slice(&info.size.to_le_bytes());
        bytes[16..24].copy_from_slice(&info.used.to_le_bytes());
        bytes[24..].copy_from_slice(&info.regions.to_le_bytes());
        frame.set_result(if self.platform.write_user(out, &bytes) {
            32
        } else {
            u64::MAX
        });
        Switch::Continue
    }

    /// 按索引枚举 System Arena 的 Region。
    ///
    /// `out == 0` 时只返回 Region 数；否则写入一条
    /// `SYSTEM_REGION_RECORD_BYTES` 记录（虚拟地址、长度、权利、标志）。
    pub(crate) fn system_arena_regions(&mut self, frame: &mut C) -> Switch {
        let index = frame.arg0() as usize;
        let out = frame.arg1();
        let capacity = frame.arg2() as usize;
        if out == 0 {
            let count = self
                .platform
                .system_arena_info()
                .map_or(u64::MAX, |info| info.regions);
            frame.set_result(count);
            return Switch::Continue;
        }
        if capacity < hk_syscall::SYSTEM_REGION_RECORD_BYTES {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let Some(record) = self.platform.system_arena_region(index) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let mut bytes = [0_u8; hk_syscall::SYSTEM_REGION_RECORD_BYTES];
        bytes[..8].copy_from_slice(&record.virt.to_le_bytes());
        bytes[8..16].copy_from_slice(&record.len.to_le_bytes());
        bytes[16..20].copy_from_slice(&record.rights.to_le_bytes());
        bytes[20..].copy_from_slice(&record.flags.to_le_bytes());
        frame.set_result(if self.platform.write_user(out, &bytes) {
            hk_syscall::SYSTEM_REGION_RECORD_BYTES as u64
        } else {
            u64::MAX
        });
        Switch::Continue
    }
}
