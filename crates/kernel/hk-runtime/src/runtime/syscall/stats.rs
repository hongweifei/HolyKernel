//! System Capacity 记账快照。

use super::*;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    /// 按索引枚举容量 Ledger；`index` 越界时返回 0。
    pub(crate) fn ledger_enum(&mut self, frame: &mut C) -> Switch {
        let out = frame.arg0();
        let capacity = frame.arg1() as usize;
        let index = frame.arg2() as usize;
        if out == 0 || capacity < hk_syscall::LEDGER_RECORD_BYTES {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let Some(stats) = self.platform.ledger_stats() else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Some(entry) = stats.get(index) else {
            frame.set_result(0);
            return Switch::Continue;
        };
        let mut bytes = [0_u8; hk_syscall::LEDGER_RECORD_BYTES];
        bytes[..8].copy_from_slice(&entry.capacity.get().to_le_bytes());
        bytes[8..16].copy_from_slice(&entry.used.to_le_bytes());
        bytes[16..24].copy_from_slice(&entry.limit.to_le_bytes());
        bytes[24..32].copy_from_slice(&entry.peak.to_le_bytes());
        bytes[32..40].copy_from_slice(&entry.pressure.to_le_bytes());
        bytes[40..48].copy_from_slice(&entry.resident.to_le_bytes());
        bytes[48..56].copy_from_slice(&entry.soft_limit.to_le_bytes());
        bytes[56..].copy_from_slice(&ledger_kind_code(entry.kind).to_le_bytes());
        frame.set_result(if self.platform.write_user(out, &bytes) {
            hk_syscall::LEDGER_RECORD_BYTES as u64
        } else {
            u64::MAX
        });
        Switch::Continue
    }

    /// 查询对象句柄关联的 Ledger。
    pub(crate) fn object_ledger(&mut self, index: usize, frame: &mut C) -> Switch {
        let out = frame.arg0();
        let capacity = frame.arg1() as usize;
        let number = frame.arg2() as usize;
        if out == 0 || capacity < 48 {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let Some(handle) = self.handle_at(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Ok(mut snapshot) = self.kernel.object_ledger(self.tasks[index].tcb, handle) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        // 装载器登记的镜像/栈能力没有 backing，常驻页由 Region 的记账表达；
        // 能力快照回落到该 Region 的 charge，避免报告恒为 0。
        if snapshot.resident == 0
            && let Some(mapping) = self
                .process(index)
                .regions
                .iter()
                .flatten()
                .find(|mapping| mapping.capability == handle)
        {
            let mut resident = mapping.charge.map_or(0, |charge| charge.amount());
            for (_, charge) in &mapping.page_charges {
                resident = resident.saturating_add(charge.amount());
            }
            snapshot.resident = resident;
        }
        let mut bytes = [0_u8; 48];
        bytes[..8].copy_from_slice(&snapshot.charged.to_le_bytes());
        bytes[8..16].copy_from_slice(&snapshot.limit.to_le_bytes());
        bytes[16..24].copy_from_slice(&snapshot.peak.to_le_bytes());
        bytes[24..32].copy_from_slice(&snapshot.pressure.to_le_bytes());
        bytes[32..40].copy_from_slice(&snapshot.resident.to_le_bytes());
        bytes[40..].copy_from_slice(&snapshot.soft_limit.to_le_bytes());
        frame.set_result(if self.platform.write_user(out, &bytes) {
            48
        } else {
            u64::MAX
        });
        Switch::Continue
    }

    /// 查询 NUMA 拓扑；`capacity == 0` 返回记录数。
    pub(crate) fn numa_info(&mut self, frame: &mut C) -> Switch {
        let out = frame.arg0();
        let capacity = frame.arg1() as usize;
        let index = frame.arg2() as usize;
        let topology = self.platform.numa_topology();
        if capacity == 0 {
            frame.set_result(topology.len() as u64);
            return Switch::Continue;
        }
        let Some((range, node)) = topology.get(index) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if out == 0 || capacity < hk_syscall::NUMA_NODE_BYTES {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let mut bytes = [0_u8; hk_syscall::NUMA_NODE_BYTES];
        bytes[..4].copy_from_slice(&u32::from(*node).to_le_bytes());
        bytes[8..16].copy_from_slice(&range.start().get().to_le_bytes());
        bytes[16..24].copy_from_slice(&range.len().to_le_bytes());
        frame.set_result(if self.platform.write_user(out, &bytes) {
            hk_syscall::NUMA_NODE_BYTES as u64
        } else {
            u64::MAX
        });
        Switch::Continue
    }

    pub(crate) fn frame_bank_stats(&mut self, frame: &mut C) -> Switch {
        let out = frame.arg0();
        let capacity = frame.arg1() as usize;
        let size = 32 + hk_syscall::CAPACITY_KIND_COUNT * 8 + hk_syscall::MAX_NUMA_NODES * 8;
        if out == 0 || capacity < size {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let Some(stats) = self.platform.frame_bank_stats() else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let mut bytes =
            [0_u8; 32 + hk_syscall::CAPACITY_KIND_COUNT * 8 + hk_syscall::MAX_NUMA_NODES * 8];
        bytes[..8].copy_from_slice(&stats.total.to_le_bytes());
        bytes[8..16].copy_from_slice(&stats.used.to_le_bytes());
        bytes[16..24].copy_from_slice(&stats.peak.to_le_bytes());
        bytes[24..32].copy_from_slice(&stats.free.to_le_bytes());
        for (index, value) in stats.by_capacity.iter().enumerate() {
            let start = 32 + index * 8;
            bytes[start..start + 8].copy_from_slice(&value.to_le_bytes());
        }
        let base = 32 + hk_syscall::CAPACITY_KIND_COUNT * 8;
        for (index, value) in stats.by_node.iter().enumerate() {
            let start = base + index * 8;
            bytes[start..start + 8].copy_from_slice(&value.to_le_bytes());
        }
        frame.set_result(if self.platform.write_user(out, &bytes) {
            size as u64
        } else {
            u64::MAX
        });
        Switch::Continue
    }
}

fn ledger_kind_code(kind: hk_mem::hmp::CapacityKind) -> u64 {
    u64::from(match kind {
        hk_mem::hmp::CapacityKind::System => hk_syscall::CAPACITY_KIND_SYSTEM,
        hk_mem::hmp::CapacityKind::User => hk_syscall::CAPACITY_KIND_USER,
        hk_mem::hmp::CapacityKind::Dma => hk_syscall::CAPACITY_KIND_DMA,
        hk_mem::hmp::CapacityKind::Gpu => hk_syscall::CAPACITY_KIND_GPU,
        hk_mem::hmp::CapacityKind::Numa(_) => hk_syscall::CAPACITY_KIND_NUMA,
    })
}
