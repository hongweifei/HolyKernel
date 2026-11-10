//! POSIX 与 Cadence 的映射基元。
//!
//! POSIX Server 负责 `nice`/`sched_*` 的 syscall 语义与准入；这里只提供
//! 可复用的 nice ↔ Cadence 权重换算，避免映射规则散落在服务实现中。

use crate::{MAX_WEIGHT, WEIGHT_REFERENCE, Weight};

/// Linux `nice` 最小值。
pub const NICE_MIN: i8 = -20;
/// Linux `nice` 最大值。
pub const NICE_MAX: i8 = 19;

/// 把 POSIX `nice` 换算为 Cadence 权重。
///
/// 以 nice 0 = 权重 1024 为基准，每降一个 nice 约提升 25%，与 CFS 的
/// 权重梯度一致；结果夹在 `[1, MAX_WEIGHT]`。
pub fn nice_to_weight(nice: i8) -> Weight {
    let nice = nice.clamp(NICE_MIN, NICE_MAX);
    let mut weight = u64::from(WEIGHT_REFERENCE);
    if nice < 0 {
        for _ in 0..-nice {
            weight = weight.saturating_mul(5) / 4;
        }
    } else {
        for _ in 0..nice {
            weight = weight.saturating_mul(4) / 5;
        }
    }
    Weight::new(weight.clamp(1, u64::from(MAX_WEIGHT)) as u32).unwrap_or(Weight::MIN)
}

/// 把 Cadence 权重换算为最接近的 POSIX `nice`。
pub fn weight_to_nice(weight: Weight) -> i8 {
    let target = u64::from(weight.get());
    let mut best = 0_i8;
    let mut best_delta = u64::MAX;
    for nice in NICE_MIN..=NICE_MAX {
        let delta = u64::from(nice_to_weight(nice).get()).abs_diff(target);
        if delta < best_delta {
            best_delta = delta;
            best = nice;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_nice_zero_to_reference_weight() {
        assert_eq!(nice_to_weight(0).get(), WEIGHT_REFERENCE);
        assert_eq!(weight_to_nice(Weight::REFERENCE), 0);
    }

    #[test]
    fn lower_nice_gets_higher_weight() {
        let mut previous = u32::MAX;
        for nice in NICE_MIN..=NICE_MAX {
            let weight = nice_to_weight(nice).get();
            assert!(weight <= previous, "权重应随 nice 增大而单调下降");
            previous = weight;
        }
        assert!(nice_to_weight(-20).get() > nice_to_weight(0).get());
        assert!(nice_to_weight(19).get() < nice_to_weight(0).get());
    }

    #[test]
    fn weight_round_trips_every_nice_value() {
        for nice in NICE_MIN..=NICE_MAX {
            assert_eq!(weight_to_nice(nice_to_weight(nice)), nice);
        }
    }

    #[test]
    fn clamps_out_of_range_nice() {
        assert_eq!(nice_to_weight(-100), nice_to_weight(NICE_MIN));
        assert_eq!(nice_to_weight(100), nice_to_weight(NICE_MAX));
    }
}
