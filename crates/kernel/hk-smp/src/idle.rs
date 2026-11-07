//! CPU 空闲与唤醒标志。
//!
//! `wake` 总是置位唤醒标志；只有返回 `true` 时平台才需要发送 IPI。这样即使
//! CPU 正在进入空闲、尚未置位 idle，也不会丢失唤醒。

use core::sync::atomic::{AtomicBool, Ordering};

/// 每 CPU 的空闲与唤醒标志。
pub struct IdleState<const N: usize> {
    idle: [AtomicBool; N],
    wake: [AtomicBool; N],
}

impl<const N: usize> IdleState<N> {
    /// 构造空状态。
    pub const fn new() -> Self {
        Self {
            idle: [const { AtomicBool::new(false) }; N],
            wake: [const { AtomicBool::new(false) }; N],
        }
    }

    /// 标记 CPU 进入空闲。
    pub fn enter(&self, cpu: usize) {
        if let Some(idle) = self.idle.get(cpu) {
            idle.store(true, Ordering::Release);
        }
    }

    /// 清除 CPU 的空闲标记。
    pub fn leave(&self, cpu: usize) {
        if let Some(idle) = self.idle.get(cpu) {
            idle.store(false, Ordering::Release);
        }
    }

    /// 判断 CPU 是否空闲。
    pub fn is_idle(&self, cpu: usize) -> bool {
        self.idle
            .get(cpu)
            .is_some_and(|idle| idle.load(Ordering::Acquire))
    }

    /// 返回空闲 CPU 数量。
    pub fn idle_count(&self) -> usize {
        self.idle
            .iter()
            .filter(|idle| idle.load(Ordering::Acquire))
            .count()
    }

    /// 设置唤醒标志；返回设置前 CPU 是否空闲。
    pub fn wake(&self, cpu: usize) -> bool {
        let Some(wake) = self.wake.get(cpu) else {
            return false;
        };
        wake.store(true, Ordering::Release);
        self.is_idle(cpu)
    }

    /// 取走并清除唤醒标志。
    pub fn take_wake(&self, cpu: usize) -> bool {
        self.wake
            .get(cpu)
            .is_some_and(|wake| wake.swap(false, Ordering::AcqRel))
    }
}

impl<const N: usize> Default for IdleState<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_before_enter_is_not_lost() {
        let state = IdleState::<2>::new();
        assert!(!state.wake(1));
        state.enter(1);
        assert!(state.take_wake(1));
        state.leave(1);
        assert_eq!(state.idle_count(), 0);
    }

    #[test]
    fn wake_after_enter_requests_ipi() {
        let state = IdleState::<2>::new();
        state.enter(1);
        assert_eq!(state.idle_count(), 1);
        assert!(state.wake(1));
        assert!(state.take_wake(1));
        assert!(!state.take_wake(1));
    }
}
