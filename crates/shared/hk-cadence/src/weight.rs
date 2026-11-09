//! 权重与虚拟时间数学。

use hk_base::Duration;

use crate::CadenceError;

/// 权重参考值：权重等于该值的 Task 获得基准时间片与 1:1 虚拟时间。
pub const WEIGHT_REFERENCE: u32 = 1024;

/// 权重上限，避免虚拟时间计算溢出。
pub const MAX_WEIGHT: u32 = 1 << 20;

/// 基准时间片（2 ms）。
pub const BASE_SLICE: Duration = Duration::from_ticks(2_000);

/// 最小时间片（1 ms）。
pub const MIN_SLICE: Duration = Duration::from_ticks(1_000);

/// 最大时间片（8 ms）。
pub const MAX_SLICE: Duration = Duration::from_ticks(8_000);

/// 调度权重。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Weight(u32);

impl Weight {
    /// 最小权重。
    pub const MIN: Self = Self(1);

    /// 基准权重。
    pub const REFERENCE: Self = Self(WEIGHT_REFERENCE);

    /// 最大权重。
    pub const MAX: Self = Self(MAX_WEIGHT);

    /// 校验并构造权重。
    pub const fn new(value: u32) -> Result<Self, CadenceError> {
        if value == 0 {
            return Err(CadenceError::ZeroWeight);
        }
        if value > MAX_WEIGHT {
            return Err(CadenceError::WeightTooLarge);
        }
        Ok(Self(value))
    }

    /// 返回权重数值。
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// 按权重计算时间片：`BASE_SLICE * WEIGHT_REFERENCE / weight`，并夹在上下限内。
pub const fn slice_for(weight: Weight) -> Duration {
    let scaled = (BASE_SLICE.ticks() as u128 * WEIGHT_REFERENCE as u128) / weight.get() as u128;
    let ticks = if scaled < MIN_SLICE.ticks() as u128 {
        MIN_SLICE.ticks()
    } else if scaled > MAX_SLICE.ticks() as u128 {
        MAX_SLICE.ticks()
    } else {
        scaled as u64
    };
    Duration::from_ticks(ticks)
}

/// 按权重把实际执行时间换算为虚拟时间：`elapsed * WEIGHT_REFERENCE / weight`。
///
/// 溢出时饱和到 `u64::MAX`。
pub const fn charge_for(elapsed: Duration, weight: Weight) -> u64 {
    let scaled = (elapsed.ticks() as u128 * WEIGHT_REFERENCE as u128) / weight.get() as u128;
    if scaled > u64::MAX as u128 {
        u64::MAX
    } else {
        scaled as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_weights() {
        assert_eq!(Weight::new(0).err(), Some(CadenceError::ZeroWeight));
        assert_eq!(
            Weight::new(MAX_WEIGHT + 1).err(),
            Some(CadenceError::WeightTooLarge)
        );
        assert_eq!(Weight::new(1), Ok(Weight::MIN));
    }

    #[test]
    fn scales_slice_inversely_with_weight() {
        assert_eq!(slice_for(Weight::REFERENCE), BASE_SLICE);
        assert_eq!(slice_for(Weight::new(2048).unwrap()), MIN_SLICE);
        assert_eq!(
            slice_for(Weight::new(512).unwrap()),
            Duration::from_ticks(4_000)
        );
        assert_eq!(slice_for(Weight::new(128).unwrap()), MAX_SLICE);
    }

    #[test]
    fn charges_virtual_time_proportionally() {
        let elapsed = Duration::from_ticks(10);
        assert_eq!(charge_for(elapsed, Weight::REFERENCE), 10);
        assert_eq!(charge_for(elapsed, Weight::new(2048).unwrap()), 5);
        assert_eq!(charge_for(elapsed, Weight::new(512).unwrap()), 20);
    }
}
