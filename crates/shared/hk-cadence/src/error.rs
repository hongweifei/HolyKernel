//! Cadence 契约错误。

use core::fmt;

/// Cadence 契约校验与操作错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CadenceError {
    /// 权重为 0。
    ZeroWeight,
    /// 权重超过上限。
    WeightTooLarge,
    /// 亲和性掩码为空。
    ZeroCpuMask,
    /// 子契约的亲和性不是父契约的子集。
    AffinityNotSubset,
    /// Realtime 契约缺少预算。
    RealtimeNeedsBudget,
    /// Realtime 契约缺少周期。
    RealtimeNeedsPeriod,
    /// 预算超过周期。
    BudgetExceedsPeriod,
    /// 设置了周期但没有预算。
    PeriodWithoutBudget,
    /// 截止期晚于周期。
    DeadlineAfterPeriod,
    /// 借贷深度超过上限。
    LendingTooDeep,
    /// 子契约的借贷策略比父契约更宽。
    LendingWidensParent,
    /// 子契约权重超过父契约。
    WeightExceedsParent,
    /// 子契约预算超过父契约。
    BudgetExceedsParent,
    /// 预算池已耗尽。
    BudgetPoolExhausted,
    /// 预算租约不属于该池。
    BudgetPoolForeign,
    /// 预算租约为零。
    ZeroBudget,
    /// 快照缓冲区不足。
    SnapshotTooShort,
    /// 快照中的类别无法识别。
    UnknownClass,
}

impl fmt::Display for CadenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroWeight => formatter.write_str("权重必须大于 0"),
            Self::WeightTooLarge => formatter.write_str("权重超过上限"),
            Self::ZeroCpuMask => formatter.write_str("亲和性掩码不能为空"),
            Self::AffinityNotSubset => formatter.write_str("亲和性必须收窄"),
            Self::RealtimeNeedsBudget => formatter.write_str("Realtime 契约必须设置预算"),
            Self::RealtimeNeedsPeriod => formatter.write_str("Realtime 契约必须设置周期"),
            Self::BudgetExceedsPeriod => formatter.write_str("预算不能超过周期"),
            Self::PeriodWithoutBudget => formatter.write_str("没有预算时不能设置周期"),
            Self::DeadlineAfterPeriod => formatter.write_str("截止期不能晚于周期"),
            Self::LendingTooDeep => formatter.write_str("借贷深度超过上限"),
            Self::LendingWidensParent => formatter.write_str("借贷策略不能比父契约更宽"),
            Self::WeightExceedsParent => formatter.write_str("子契约权重不能超过父契约"),
            Self::BudgetExceedsParent => formatter.write_str("子契约预算不能超过父契约"),
            Self::BudgetPoolExhausted => formatter.write_str("预算池已耗尽"),
            Self::BudgetPoolForeign => formatter.write_str("预算租约不属于该池"),
            Self::ZeroBudget => formatter.write_str("预算租约不能为零"),
            Self::SnapshotTooShort => formatter.write_str("快照缓冲区不足"),
            Self::UnknownClass => formatter.write_str("快照中的调度类别无法识别"),
        }
    }
}

impl core::error::Error for CadenceError {}
