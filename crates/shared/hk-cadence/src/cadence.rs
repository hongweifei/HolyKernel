//! Cadence 时间契约。

use core::sync::atomic::{AtomicU64, Ordering};

use hk_base::Duration;

use crate::weight::WEIGHT_REFERENCE;
use crate::{CadenceClass, CadenceError, CpuMask, Lending, Weight};

static NEXT_CADENCE_ID: AtomicU64 = AtomicU64::new(1);

/// Cadence 标识。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CadenceId(u64);

impl CadenceId {
    /// 以数值构造标识。
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// 分配一个进程内唯一标识。
    pub fn next() -> Self {
        Self(NEXT_CADENCE_ID.fetch_add(1, Ordering::Relaxed))
    }

    /// 返回标识数值。
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// 可静态构造的 Cadence 描述。
///
/// 服务清单与 Root Server 的默认授予使用它；权重与亲和性以原始值保存，由
/// [`Cadence::from_spec`] 统一校验。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CadenceSpec {
    /// 调度类别。
    pub class: CadenceClass,
    /// 权重。
    pub weight: u32,
    /// 周期内预算；0 表示纯权重分享。
    pub budget: Duration,
    /// 预算周期。
    pub period: Duration,
    /// 相对截止期；0 表示按周期。
    pub deadline: Duration,
    /// 延迟目标。
    pub latency: Duration,
    /// CPU 亲和性。
    pub affinity: CpuMask,
    /// 是否参与跨 CPU 迁移。
    pub migratable: bool,
    /// 借贷策略。
    pub lending: Lending,
}

impl CadenceSpec {
    /// 以类别与权重构造描述，其余字段取默认值。
    pub const fn new(class: CadenceClass, weight: u32) -> Self {
        Self {
            class,
            weight,
            budget: Duration::ZERO,
            period: Duration::ZERO,
            deadline: Duration::ZERO,
            latency: Duration::ZERO,
            affinity: CpuMask::ALL,
            migratable: true,
            lending: Lending::caller_pays(),
        }
    }

    /// 设置预算与周期。
    pub const fn with_budget(mut self, budget: Duration, period: Duration) -> Self {
        self.budget = budget;
        self.period = period;
        self
    }

    /// 设置相对截止期。
    pub const fn with_deadline(mut self, deadline: Duration) -> Self {
        self.deadline = deadline;
        self
    }

    /// 设置延迟目标。
    pub const fn with_latency(mut self, latency: Duration) -> Self {
        self.latency = latency;
        self
    }

    /// 设置 CPU 亲和性。
    pub const fn with_affinity(mut self, affinity: CpuMask) -> Self {
        self.affinity = affinity;
        self
    }

    /// 设置是否参与迁移。
    pub const fn with_migratable(mut self, migratable: bool) -> Self {
        self.migratable = migratable;
        self
    }

    /// 设置借贷策略。
    pub const fn with_lending(mut self, lending: Lending) -> Self {
        self.lending = lending;
        self
    }

    /// 以标识构造并校验契约。
    pub fn cadence(self, id: CadenceId) -> Result<Cadence, CadenceError> {
        Cadence::from_spec(id, self)
    }
}

/// 默认契约：Latency 类、基准权重、无硬预算。
pub const DEFAULT_SPEC: CadenceSpec = CadenceSpec::new(CadenceClass::Latency, WEIGHT_REFERENCE);

/// 利用率固定点标度（1.0 = 1_000_000）。
pub const UTILIZATION_SCALE: u64 = 1_000_000;

/// 计算 `budget / period` 的固定点利用率。
pub fn utilization(budget: Duration, period: Duration) -> u64 {
    if period.ticks() == 0 {
        return 0;
    }
    (budget.ticks().saturating_mul(UTILIZATION_SCALE) / period.ticks()).min(UTILIZATION_SCALE)
}

/// 计算 `budget / deadline` 的固定点密度；deadline 为 0 时按 period。
pub fn density(budget: Duration, deadline: Duration, period: Duration) -> u64 {
    let limit = if deadline.ticks() == 0 {
        period.ticks()
    } else {
        deadline.ticks()
    };
    if limit == 0 {
        return 0;
    }
    (budget.ticks().saturating_mul(UTILIZATION_SCALE) / limit).min(UTILIZATION_SCALE)
}

/// Realtime 准入：类别必须是 Realtime，且累计 EDF 密度不超过 1.0。
pub fn admits_realtime(used: u64, spec: CadenceSpec) -> bool {
    spec.class == CadenceClass::Realtime
        && used.saturating_add(density(spec.budget, spec.deadline, spec.period))
            <= UTILIZATION_SCALE
}

/// 空闲契约：Idle 类、最小权重。
pub const IDLE_SPEC: CadenceSpec = CadenceSpec::new(CadenceClass::Idle, 1);

/// 时间契约。
///
/// 类别、权重、预算、周期、截止期、延迟目标、亲和性与借贷策略都由契约表达；
/// 内核 Beat 只执行，Tempo Server 决定授予与调整。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cadence {
    id: CadenceId,
    parent: Option<CadenceId>,
    epoch: u32,
    class: CadenceClass,
    weight: Weight,
    budget: Duration,
    period: Duration,
    deadline: Duration,
    latency: Duration,
    affinity: CpuMask,
    migratable: bool,
    lending: Lending,
    children_budget: u64,
}

impl Cadence {
    /// 以类别与权重构造默认契约。
    pub fn new(id: CadenceId, class: CadenceClass, weight: Weight) -> Self {
        Self {
            id,
            parent: None,
            epoch: 0,
            class,
            weight,
            budget: Duration::ZERO,
            period: Duration::ZERO,
            deadline: Duration::ZERO,
            latency: Duration::ZERO,
            affinity: CpuMask::ALL,
            migratable: true,
            lending: Lending::caller_pays(),
            children_budget: 0,
        }
    }

    /// 以描述构造并校验契约。
    pub fn from_spec(id: CadenceId, spec: CadenceSpec) -> Result<Self, CadenceError> {
        Self::restore(id, None, 0, spec)
    }

    /// 以描述、父契约与 epoch 还原契约；checkpoint 恢复使用。
    pub fn restore(
        id: CadenceId,
        parent: Option<CadenceId>,
        epoch: u32,
        spec: CadenceSpec,
    ) -> Result<Self, CadenceError> {
        let cadence = Self {
            id,
            parent,
            epoch,
            class: spec.class,
            weight: Weight::new(spec.weight)?,
            budget: spec.budget,
            period: spec.period,
            deadline: spec.deadline,
            latency: spec.latency,
            affinity: spec.affinity,
            migratable: spec.migratable,
            lending: spec.lending,
            children_budget: 0,
        };
        cadence.validate()?;
        Ok(cadence)
    }

    /// 设置预算与周期。
    pub fn set_budget(&mut self, budget: Duration, period: Duration) {
        self.budget = budget;
        self.period = period;
    }

    /// 设置相对截止期。
    pub fn set_deadline(&mut self, deadline: Duration) {
        self.deadline = deadline;
    }

    /// 设置延迟目标。
    pub fn set_latency(&mut self, latency: Duration) {
        self.latency = latency;
    }

    /// 设置亲和性。
    pub fn set_affinity(&mut self, affinity: CpuMask) {
        self.affinity = affinity;
    }

    /// 设置是否参与迁移。
    pub fn set_migratable(&mut self, migratable: bool) {
        self.migratable = migratable;
    }

    /// 设置借贷策略。
    pub fn set_lending(&mut self, lending: Lending) {
        self.lending = lending;
    }

    /// 校验契约不变量。
    pub fn validate(&self) -> Result<(), CadenceError> {
        if self.affinity.is_empty() {
            return Err(CadenceError::ZeroCpuMask);
        }
        if self.weight.get() == 0 {
            return Err(CadenceError::ZeroWeight);
        }
        if !self.deadline.is_zero() && (self.period.is_zero() || self.deadline > self.period) {
            return Err(CadenceError::DeadlineAfterPeriod);
        }
        match self.class {
            CadenceClass::Realtime => {
                if self.budget.is_zero() {
                    return Err(CadenceError::RealtimeNeedsBudget);
                }
                if self.period.is_zero() {
                    return Err(CadenceError::RealtimeNeedsPeriod);
                }
                if self.budget > self.period {
                    return Err(CadenceError::BudgetExceedsPeriod);
                }
            }
            CadenceClass::Latency | CadenceClass::Throughput => {
                if self.budget.is_zero() {
                    if !self.period.is_zero() {
                        return Err(CadenceError::PeriodWithoutBudget);
                    }
                } else if self.period.is_zero() || self.budget > self.period {
                    return Err(CadenceError::BudgetExceedsPeriod);
                }
            }
            CadenceClass::Idle => {}
        }
        Ok(())
    }

    /// 从本契约派生子契约；子契约只收窄权重、预算与借贷。
    pub fn split(
        &mut self,
        id: CadenceId,
        weight: Weight,
        budget: Duration,
    ) -> Result<Self, CadenceError> {
        if weight > self.weight {
            return Err(CadenceError::WeightExceedsParent);
        }
        if self.budget.is_zero() {
            if !budget.is_zero() {
                return Err(CadenceError::BudgetExceedsParent);
            }
        } else if budget > self.budget
            || self.children_budget.saturating_add(budget.ticks()) > self.budget.ticks()
        {
            return Err(CadenceError::BudgetExceedsParent);
        }
        let spec = CadenceSpec {
            class: self.class,
            weight: weight.get(),
            budget,
            period: self.period,
            deadline: self.deadline,
            latency: self.latency,
            affinity: self.affinity,
            migratable: self.migratable,
            lending: self.lending.narrowed_to(self.lending)?,
        };
        let child = Self::restore(id, Some(self.id), self.epoch, spec)?;
        self.children_budget = self.children_budget.saturating_add(budget.ticks());
        Ok(child)
    }

    /// 返回契约描述。
    pub fn spec(&self) -> CadenceSpec {
        CadenceSpec {
            class: self.class,
            weight: self.weight.get(),
            budget: self.budget,
            period: self.period,
            deadline: self.deadline,
            latency: self.latency,
            affinity: self.affinity,
            migratable: self.migratable,
            lending: self.lending,
        }
    }

    /// 返回已派生子契约占用的预算总和。
    pub const fn children_budget(&self) -> u64 {
        self.children_budget
    }

    /// 归还一个已释放子契约的预算。
    pub fn release_child_budget(&mut self, budget: u64) {
        self.children_budget = self.children_budget.saturating_sub(budget);
    }

    /// 预留一个子契约的预算；更新子契约预算时与 [`Self::release_child_budget`] 配对使用。
    pub fn reserve_child_budget(&mut self, budget: u64) {
        self.children_budget = self.children_budget.saturating_add(budget);
    }

    /// 解除父链，把收窄契约提升为独立契约。
    ///
    /// 父契约撤销或父 Task 退出后使用；契约的类别、权重、预算与亲和性保持不变。
    pub fn promote(&mut self) {
        self.parent = None;
    }

    /// 递增 epoch，使旧句柄失效；返回新 epoch。
    pub fn revoke(&mut self) -> u32 {
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.epoch = 1;
        }
        self.epoch
    }

    /// 判断 epoch 是否仍有效。
    pub const fn is_current(&self, epoch: u32) -> bool {
        self.epoch == epoch
    }

    /// 返回契约标识。
    pub const fn id(&self) -> CadenceId {
        self.id
    }

    /// 返回父契约。
    pub const fn parent(&self) -> Option<CadenceId> {
        self.parent
    }

    /// 返回当前 epoch。
    pub const fn epoch(&self) -> u32 {
        self.epoch
    }

    /// 返回类别。
    pub const fn class(&self) -> CadenceClass {
        self.class
    }

    /// 返回权重。
    pub const fn weight(&self) -> Weight {
        self.weight
    }

    /// 返回预算。
    pub const fn budget(&self) -> Duration {
        self.budget
    }

    /// 返回周期。
    pub const fn period(&self) -> Duration {
        self.period
    }

    /// 返回截止期。
    pub const fn deadline(&self) -> Duration {
        self.deadline
    }

    /// 返回延迟目标。
    pub const fn latency(&self) -> Duration {
        self.latency
    }

    /// 返回亲和性。
    pub const fn affinity(&self) -> CpuMask {
        self.affinity
    }

    /// 判断是否参与迁移。
    pub const fn migratable(&self) -> bool {
        self.migratable
    }

    /// 返回借贷策略。
    pub const fn lending(&self) -> Lending {
        self.lending
    }

    /// 判断是否带硬预算。
    pub const fn has_budget(&self) -> bool {
        !self.budget.is_zero()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weight(value: u32) -> Weight {
        Weight::new(value).expect("权重合法")
    }

    #[test]
    fn builds_and_validates_weight_shared_cadence() {
        let cadence = Cadence::new(CadenceId::new(1), CadenceClass::Latency, weight(2048));
        assert_eq!(cadence.validate(), Ok(()));
        assert!(!cadence.has_budget());
        assert_eq!(cadence.affinity(), CpuMask::ALL);
        assert!(cadence.migratable());
    }

    #[test]
    fn rejects_empty_affinity_and_zero_weight_specs() {
        let empty = CadenceSpec {
            affinity: CpuMask::empty(),
            ..CadenceSpec::new(CadenceClass::Latency, 1024)
        };
        assert_eq!(
            Cadence::from_spec(CadenceId::new(1), empty).err(),
            Some(CadenceError::ZeroCpuMask)
        );

        let zero = CadenceSpec::new(CadenceClass::Latency, 0);
        assert_eq!(
            Cadence::from_spec(CadenceId::new(1), zero).err(),
            Some(CadenceError::ZeroWeight)
        );
    }

    #[test]
    fn realtime_requires_budget_and_period() {
        let missing = CadenceSpec::new(CadenceClass::Realtime, 1024);
        assert_eq!(
            Cadence::from_spec(CadenceId::new(1), missing).err(),
            Some(CadenceError::RealtimeNeedsBudget)
        );

        let over_budget = CadenceSpec::new(CadenceClass::Realtime, 1024)
            .with_budget(Duration::from_ticks(5), Duration::from_ticks(4));
        assert_eq!(
            Cadence::from_spec(CadenceId::new(1), over_budget).err(),
            Some(CadenceError::BudgetExceedsPeriod)
        );

        let valid = CadenceSpec::new(CadenceClass::Realtime, 1024)
            .with_budget(Duration::from_ticks(3), Duration::from_ticks(16))
            .with_deadline(Duration::from_ticks(8));
        assert!(Cadence::from_spec(CadenceId::new(1), valid).is_ok());
    }

    #[test]
    fn rejects_period_without_budget_and_late_deadline() {
        let period_only = CadenceSpec::new(CadenceClass::Throughput, 1024)
            .with_budget(Duration::ZERO, Duration::from_ticks(4));
        assert_eq!(
            Cadence::from_spec(CadenceId::new(1), period_only).err(),
            Some(CadenceError::PeriodWithoutBudget)
        );

        let late = CadenceSpec::new(CadenceClass::Latency, 1024)
            .with_latency(Duration::from_ticks(2))
            .with_deadline(Duration::from_ticks(4));
        assert_eq!(
            Cadence::from_spec(CadenceId::new(1), late).err(),
            Some(CadenceError::DeadlineAfterPeriod)
        );
    }

    #[test]
    fn releases_child_budget() {
        let mut parent = CadenceSpec::new(CadenceClass::Latency, 4096)
            .with_budget(Duration::from_ticks(8), Duration::from_ticks(32))
            .cadence(CadenceId::new(1))
            .expect("父契约");
        parent
            .split(CadenceId::new(2), weight(1024), Duration::from_ticks(4))
            .expect("子契约");
        assert_eq!(parent.children_budget(), 4);
        parent.release_child_budget(4);
        assert_eq!(parent.children_budget(), 0);
        parent
            .split(CadenceId::new(3), weight(1024), Duration::from_ticks(7))
            .expect("释放后可重新分配");
    }

    #[test]
    fn computes_realtime_utilization_and_admission() {
        let spec = CadenceSpec::new(CadenceClass::Realtime, 1024)
            .with_budget(Duration::from_ticks(2_000), Duration::from_ticks(16_000));
        assert_eq!(
            utilization(Duration::from_ticks(2_000), Duration::from_ticks(16_000)),
            125_000
        );
        assert!(admits_realtime(0, spec));
        assert!(admits_realtime(875_000, spec));
        assert!(!admits_realtime(900_000, spec));
        assert!(!admits_realtime(
            0,
            CadenceSpec::new(CadenceClass::Latency, 1024)
        ));
    }

    #[test]
    fn splits_only_narrowing() {
        let mut parent = CadenceSpec::new(CadenceClass::Latency, 4096)
            .with_budget(Duration::from_ticks(8), Duration::from_ticks(32))
            .cadence(CadenceId::new(1))
            .expect("父契约");

        let child = parent
            .split(CadenceId::new(2), weight(1024), Duration::from_ticks(2))
            .expect("子契约");
        assert_eq!(child.parent(), Some(parent.id()));
        assert_eq!(child.epoch(), parent.epoch());
        assert_eq!(child.class(), parent.class());

        assert_eq!(
            parent
                .split(CadenceId::new(3), weight(8192), Duration::from_ticks(2))
                .err(),
            Some(CadenceError::WeightExceedsParent)
        );
        assert_eq!(
            parent
                .split(CadenceId::new(4), weight(1024), Duration::from_ticks(16))
                .err(),
            Some(CadenceError::BudgetExceedsParent)
        );
        assert_eq!(
            parent
                .split(CadenceId::new(5), weight(1024), Duration::from_ticks(7))
                .err(),
            Some(CadenceError::BudgetExceedsParent)
        );
    }

    #[test]
    fn revoke_invalidates_previous_epoch() {
        let mut cadence = Cadence::new(CadenceId::new(1), CadenceClass::Latency, weight(1024));
        let old = cadence.epoch();
        let new = cadence.revoke();
        assert_eq!(new, old + 1);
        assert!(!cadence.is_current(old));
        assert!(cadence.is_current(new));
    }
}
