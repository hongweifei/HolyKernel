//! 虚拟时钟与定时器。

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use hk_base::{Duration, Moment};
use hk_hal::Timer;

/// 定时器频率：1 GHz，即 1 tick 等于 1 纳秒。
pub const HOSTED_TIMER_FREQUENCY: u64 = 1_000_000_000;

/// Hosted 平台的单调时钟。
#[derive(Debug)]
pub struct HostedTimer {
    clock: Clock,
    oneshot: Mutex<Option<Moment>>,
}

#[derive(Debug)]
enum Clock {
    Real { origin: Instant },
    Virtual { ticks: AtomicU64 },
}

impl HostedTimer {
    /// 构造定时器；`deterministic` 为真时使用虚拟时钟。
    pub fn new(deterministic: bool) -> Self {
        let clock = if deterministic {
            Clock::Virtual {
                ticks: AtomicU64::new(0),
            }
        } else {
            Clock::Real {
                origin: Instant::now(),
            }
        };
        Self {
            clock,
            oneshot: Mutex::new(None),
        }
    }

    /// 推进虚拟时钟。
    ///
    /// # Panics
    /// 真实时钟按系统时间前进，调用本方法属于误用。
    pub fn advance(&self, duration: Duration) {
        match &self.clock {
            Clock::Virtual { ticks } => {
                ticks.fetch_add(duration.ticks(), Ordering::SeqCst);
            }
            Clock::Real { .. } => panic!("真实时钟不接受时间推进"),
        }
    }

    /// 返回一次性定时器的截止时间。
    pub fn oneshot_deadline(&self) -> Option<Moment> {
        *self.oneshot.lock().expect("定时器锁中毒")
    }
}

impl Timer for HostedTimer {
    fn frequency(&self) -> u64 {
        HOSTED_TIMER_FREQUENCY
    }

    fn now(&self) -> Moment {
        match &self.clock {
            Clock::Real { origin } => Moment::from_ticks(origin.elapsed().as_nanos() as u64),
            Clock::Virtual { ticks } => Moment::from_ticks(ticks.load(Ordering::SeqCst)),
        }
    }

    fn set_oneshot(&self, deadline: Moment) {
        *self.oneshot.lock().expect("定时器锁中毒") = Some(deadline);
    }

    fn cancel(&self) {
        *self.oneshot.lock().expect("定时器锁中毒") = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_clock_advances() {
        let timer = HostedTimer::new(true);
        assert_eq!(timer.now().ticks(), 0);

        timer.advance(Duration::from_ticks(1_500));
        assert_eq!(timer.now().ticks(), 1_500);
    }

    #[test]
    fn oneshot_can_be_set_and_cancelled() {
        let timer = HostedTimer::new(true);
        let deadline = Moment::from_ticks(42);

        timer.set_oneshot(deadline);
        assert_eq!(timer.oneshot_deadline(), Some(deadline));

        timer.cancel();
        assert_eq!(timer.oneshot_deadline(), None);
    }

    #[test]
    fn frequency_is_one_ghz() {
        let timer = HostedTimer::new(false);
        assert_eq!(timer.frequency(), HOSTED_TIMER_FREQUENCY);
    }
}
