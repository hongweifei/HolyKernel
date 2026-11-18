//! OOM 判定：压力度量、当前任务回收与 victim 选择。

use crate::runtime::oom;
use crate::runtime::{UserPlatform, UserRuntime};
use alloc::string::String;
use hk_hal::{SyscallFrame, UserContext};

/// 用户态 OOM 退出码。
pub const OOM_EXIT: u64 = 0x4F_4F_4D;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    /// 返回 `(free, total)`；平台没有物理统计时返回 `None`。
    pub fn memory_pressure(&self) -> Option<(u64, u64)> {
        let stats = self.platform.frame_bank_stats()?;
        (stats.total != 0).then_some((stats.free, stats.total))
    }

    /// 返回空闲百分比。
    pub fn free_percent(&self) -> Option<u64> {
        self.memory_pressure()
            .map(|(free, total)| free.saturating_mul(100) / total)
    }

    /// 主动回收当前任务的匿名页。
    pub fn reclaim_current(&mut self, index: usize, budget: u64) -> u64 {
        self.reclaim_anonymous_pages(index, budget)
    }

    /// 分配失败后的压力处理；返回 `true` 表示调用方可以重试分配。
    ///
    /// 先回收当前任务；仍低于阈值且策略允许时，按 Ledger 用量选择 victim。
    /// 当前任务被选中时直接走 fault 退出；其他任务走正常终止路径。
    pub fn oom_guard(&mut self, cpu: usize, index: usize, request: u64) -> bool {
        let Some((_, _total)) = self.memory_pressure() else {
            return false;
        };
        let threshold = u64::from(self.oom.policy.min_free_percent);
        let before = self.free_percent().unwrap_or(100);
        if before > threshold {
            return false;
        }
        let budget = request.max(hk_hal::PAGE_SIZE as u64);
        let reclaimed = self.reclaim_current(index, budget);
        let after = self.free_percent().unwrap_or(100);
        if after > threshold {
            return true;
        }
        if self.oom.policy.action == 0 {
            return false;
        }
        let current_name = self.tasks[index].name.clone();
        let mut victim: Option<(u64, usize, String)> = None;
        for (slot, task) in self.tasks.iter().enumerate() {
            if task.exited || task.leader != slot || oom::OomState::exempt(&task.name) {
                continue;
            }
            let charge = self.process(slot).ledger.charged();
            if victim.as_ref().is_none_or(|(best, _, _)| charge > *best) {
                victim = Some((charge, slot, task.name.clone()));
            }
        }
        let Some((_, slot, name)) = victim else {
            return false;
        };
        // 当前任务若本身就是最大占用者，也按同一策略退出；否则只终止 victim。
        let killed = if slot == index || name == current_name {
            self.fault(cpu, OOM_EXIT)
        } else {
            self.terminate(&name, OOM_EXIT)
        };
        self.oom.events = self.oom.events.saturating_add(1);
        if killed {
            self.oom.kills = self.oom.kills.saturating_add(1);
        }
        self.oom.last = Some(oom::OomEvent {
            victim: name,
            free_percent: after,
            reclaimed,
            killed,
        });
        self.oom
            .record_event(oom::MEMORY_EVENT_OOM, 0, 0, reclaimed, slot as u64, after);
        self.notify_memory_event(oom::MEMORY_EVENT_OOM);
        // 当前任务被终止后不能再重试；其他 victim 终止后允许一次重试。
        killed && slot != index
    }
}

#[cfg(test)]
#[allow(clippy::wildcard_imports)]
mod tests {
    use super::*;

    #[test]
    fn exit_code_is_stable() {
        assert_eq!(OOM_EXIT, 0x4F_4F_4D);
    }
}
