//! SMP AP 停泊与在线计数。

use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

use spin::Mutex;

/// AP 停泊池。
pub struct ApPool {
    online: AtomicUsize,
    ids: Mutex<Vec<u32>>,
}

impl ApPool {
    /// 构造空池。
    pub const fn new() -> Self {
        Self {
            online: AtomicUsize::new(0),
            ids: Mutex::new(Vec::new()),
        }
    }

    /// 返回在线 AP 数量。
    pub fn online(&self) -> usize {
        self.online.load(Ordering::Acquire)
    }

    /// 返回在线 AP 的本地 APIC 编号。
    pub fn ids(&self) -> Vec<u32> {
        self.ids.lock().clone()
    }

    /// 记录 APIC 编号并递增在线计数。
    pub fn mark_online(&self, lapic_id: u32) {
        self.ids.lock().push(lapic_id);
        self.online.fetch_add(1, Ordering::AcqRel);
    }

    /// 递增在线计数并停泊；不返回。
    pub fn park(&self) -> ! {
        self.online.fetch_add(1, Ordering::AcqRel);
        loop {
            core::hint::spin_loop();
        }
    }

    /// 记录 APIC 编号、递增在线计数，并以 `idle` 停泊；不返回。
    pub fn park_with<F>(&self, lapic_id: u32, mut idle: F) -> !
    where
        F: FnMut(),
    {
        self.mark_online(lapic_id);
        loop {
            idle();
        }
    }

    /// 等待在线数量达到 `expected`；超时返回当前数量。
    pub fn wait<F>(&self, expected: usize, mut now: F, timeout: u64) -> usize
    where
        F: FnMut() -> u64,
    {
        let begin = now();
        while self.online() < expected && now().wrapping_sub(begin) < timeout {
            core::hint::spin_loop();
        }
        self.online()
    }
}

impl Default for ApPool {
    fn default() -> Self {
        Self::new()
    }
}

/// 全局 AP 池。
pub static AP_POOL: ApPool = ApPool::new();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_until_timeout_for_missing_aps() {
        let pool = ApPool::new();
        let mut ticks = 0_u64;
        let online = pool.wait(
            1,
            || {
                ticks += 5;
                ticks
            },
            20,
        );
        assert_eq!(online, 0);
        assert_eq!(pool.wait(0, || 0, 0), 0);
        assert!(pool.ids().is_empty());
    }
}
