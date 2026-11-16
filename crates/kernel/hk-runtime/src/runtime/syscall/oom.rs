//! 内存压力、OOM 策略与主动回收系统调用。

use super::*;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    pub(crate) fn memory_pressure_query(&mut self, frame: &mut C) -> Switch {
        let out = frame.arg0();
        let capacity = frame.arg1() as usize;
        let Some((free, total)) = self.memory_pressure() else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        if capacity < 9 * 8 {
            frame.set_result(0);
            return Switch::Continue;
        }
        let nodes = usize::from(self.platform.numa_node_count().max(1));
        let mut bytes = alloc::vec![0_u8; 9 * 8 + nodes * 8];
        let values = [
            free,
            total,
            u64::from(self.oom.policy.min_free_percent),
            self.oom.events,
            self.oom.kills,
            self.oom.reclaimed,
            self.oom.reclaims,
            self.oom.swapped,
            self.oom.migrated,
        ];
        for (index, value) in values.iter().enumerate() {
            bytes[index * 8..index * 8 + 8].copy_from_slice(&value.to_le_bytes());
        }
        for node in 0..nodes {
            let free = self.platform.numa_free(node as u8).unwrap_or(0);
            let start = 9 * 8 + node * 8;
            bytes[start..start + 8].copy_from_slice(&free.to_le_bytes());
        }
        let bytes = &bytes[..bytes.len().min(capacity)];
        frame.set_result(if self.platform.write_user(out, bytes) {
            bytes.len() as u64
        } else {
            u64::MAX
        });
        Switch::Continue
    }

    pub(crate) fn oom_policy(&mut self, index: usize, frame: &mut C) -> Switch {
        let test_provider = self.tasks[index].name == hk_syscall::MEMORY_SERVICE
            && self
                .platform
                .boot_cmdline()
                .is_some_and(|line| line.contains("swap-test"));
        let test_oom = self.tasks[index].name == "hk-server-oom-test"
            && self
                .platform
                .boot_cmdline()
                .is_some_and(|line| line.contains("oom-test"));
        if self.tasks[index].name != "root" && !test_provider && !test_oom {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let threshold = frame.arg0();
        let action = frame.arg1();
        if threshold > 100 || action > 1 {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        self.oom.policy.min_free_percent = threshold as u8;
        self.oom.policy.action = action as u8;
        frame.set_result(0);
        Switch::Continue
    }

    pub(crate) fn oom_event_query(&mut self, frame: &mut C) -> Switch {
        let out = frame.arg0();
        let capacity = frame.arg1() as usize;
        let Some(event) = self.oom.last.as_ref() else {
            frame.set_result(0);
            return Switch::Continue;
        };
        if capacity < 5 * 8 {
            frame.set_result(0);
            return Switch::Continue;
        }
        let mut bytes = alloc::vec![0_u8; capacity.min(5 * 8 + event.victim.len())];
        let values = [
            event.free_percent,
            event.reclaimed,
            u64::from(event.killed),
            self.oom.events,
            self.oom.kills,
        ];
        for (index, value) in values.iter().enumerate() {
            bytes[index * 8..index * 8 + 8].copy_from_slice(&value.to_le_bytes());
        }
        let name = event.victim.as_bytes();
        let name_len = name.len().min(bytes.len().saturating_sub(5 * 8));
        bytes[5 * 8..5 * 8 + name_len].copy_from_slice(&name[..name_len]);
        frame.set_result(if self.platform.write_user(out, &bytes) {
            bytes.len() as u64
        } else {
            u64::MAX
        });
        Switch::Continue
    }

    pub(crate) fn memory_events_query(&mut self, frame: &mut C) -> Switch {
        let out = frame.arg0();
        let capacity = frame.arg1() as usize;
        let count = self.oom.memory_event_count;
        if capacity == 0 {
            frame.set_result(count as u64);
            return Switch::Continue;
        }
        let records = (capacity / hk_syscall::MEMORY_EVENT_BYTES).min(count);
        let mut bytes = alloc::vec![0_u8; records * hk_syscall::MEMORY_EVENT_BYTES];
        for (index, event) in self.oom.events().take(records).enumerate() {
            let values = [
                event.sequence,
                u64::from(event.kind),
                event.object,
                event.offset,
                event.length,
                event.task,
                event.value,
            ];
            for (position, value) in values.iter().enumerate() {
                let start = index * hk_syscall::MEMORY_EVENT_BYTES + position * 8;
                bytes[start..start + 8].copy_from_slice(&value.to_le_bytes());
            }
        }
        frame.set_result(if self.platform.write_user(out, &bytes) {
            bytes.len() as u64
        } else {
            u64::MAX
        });
        Switch::Continue
    }

    pub(crate) fn reclaim(&mut self, index: usize, frame: &mut C) -> Switch {
        let budget = frame.arg0().max(hk_hal::PAGE_SIZE as u64);
        let released = self.reclaim_current(index, budget);
        frame.set_result(released);
        Switch::Continue
    }
}
