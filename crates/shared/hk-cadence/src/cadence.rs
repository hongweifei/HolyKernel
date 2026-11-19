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

/// EDF 需求界函数（demand bound function）检查的时间上界（契约 tick，1 秒）。
pub const ADMISSION_HORIZON_TICKS: u64 = 1_000_000;

/// 以 demand bound function 精确判定一组 Realtime 契约在 EDF 下是否可调度。
///
/// 每项为 `(budget, period, deadline)` 契约 tick；`deadline = 0` 视为等于周期。
/// 对每个任务的所有绝对截止期 `d + k·T`（在 [`ADMISSION_HORIZON_TICKS`] 内）
/// 检查累计需求不超过时刻 `t`。约束截止期任务也可正确判定，比密度判据更精确。
pub fn edf_admissible(tasks: &[(u64, u64, u64)]) -> bool {
    if tasks
        .iter()
        .any(|(budget, period, _)| *budget == 0 || *period == 0)
    {
        return false;
    }
    for (_, period, deadline) in tasks {
        let deadline = if *deadline == 0 { *period } else { *deadline };
        let mut at = deadline;
        loop {
            if demand_bound(tasks, at) > at {
                return false;
            }
            match at.checked_add(*period) {
                Some(next) if next <= ADMISSION_HORIZON_TICKS => at = next,
                _ => break,
            }
        }
    }
    true
}

/// 计算时刻 `at` 前必须完成的累计执行需求。
fn demand_bound(tasks: &[(u64, u64, u64)], at: u64) -> u64 {
    tasks
        .iter()
        .fold(0_u64, |total, (budget, period, deadline)| {
            let deadline = if *deadline == 0 { *period } else { *deadline };
            if at < deadline {
                total
            } else {
                let jobs = (at - deadline) / period + 1;
                total.saturating_add(budget.saturating_mul(jobs))
            }
        })
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
    /// 已在父契约 `children_budget` 中预留的额度；0 表示尚未预留。
    ///
    /// 内核在把带父链的契约绑定到 Task 时预留，释放与变更时按该值归还，
    /// 避免重复扣减或误扣兄弟契约的预留。
    reserved: u64,
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
            reserved: 0,
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
            reserved: 0,
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

    /// 从本契约派生子契约；权重与预算只能收窄。
    ///
    /// 类别、周期、截止期、延迟、亲和性、迁移资格与借贷策略继承父契约。
    /// 父预算在子契约绑定 Task 时由 [`Self::reserve_in_parent`] 预留，因此
    /// 未绑定的子契约不占用父预算。
    pub fn split(
        &mut self,
        id: CadenceId,
        weight: Weight,
        budget: Duration,
    ) -> Result<Self, CadenceError> {
        let spec = CadenceSpec {
            class: self.class,
            weight: weight.get(),
            budget,
            period: self.period,
            deadline: self.deadline,
            latency: self.latency,
            affinity: self.affinity,
            migratable: self.migratable,
            lending: self.lending,
        };
        self.split_narrowed(id, spec)
    }

    /// 以完整描述派生子契约；每个字段都必须落在父契约范围内。
    ///
    /// 权重与预算收窄；类别不能强于父契约；周期与父契约一致；截止期不晚于
    /// 父契约的有效截止期；延迟目标不宽于父契约；亲和性是父集合的子集；
    /// 迁移资格不宽于父契约；借贷策略按父策略收窄。
    pub fn split_narrowed(
        &mut self,
        id: CadenceId,
        spec: CadenceSpec,
    ) -> Result<Self, CadenceError> {
        if spec.weight > self.weight.get() {
            return Err(CadenceError::WeightExceedsParent);
        }
        if spec.class < self.class {
            return Err(CadenceError::ClassExceedsParent);
        }
        spec.affinity.restrict(self.affinity)?;
        spec.lending.narrowed_to(self.lending)?;
        if spec.migratable && !self.migratable {
            return Err(CadenceError::MigratableWidensParent);
        }
        if self.period.is_zero() {
            if !spec.period.is_zero() {
                return Err(CadenceError::PeriodNotNarrowing);
            }
        } else if spec.period != self.period {
            return Err(CadenceError::PeriodNotNarrowing);
        }
        let parent_deadline = if self.deadline.is_zero() {
            self.period
        } else {
            self.deadline
        };
        let child_deadline = if spec.deadline.is_zero() {
            spec.period
        } else {
            spec.deadline
        };
        if child_deadline > parent_deadline {
            return Err(CadenceError::DeadlineExceedsParent);
        }
        if spec.latency > self.latency {
            return Err(CadenceError::LatencyExceedsParent);
        }
        if self.budget.is_zero() {
            if !spec.budget.is_zero() {
                return Err(CadenceError::BudgetExceedsParent);
            }
        } else if spec.budget > self.budget
            || self.children_budget.saturating_add(spec.budget.ticks()) > self.budget.ticks()
        {
            return Err(CadenceError::BudgetExceedsParent);
        }
        Self::restore(id, Some(self.id), self.epoch, spec)
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

    /// 返回本契约已在父契约中预留的额度。
    pub const fn reserved(&self) -> u64 {
        self.reserved
    }

    /// 取出预留额度并清零；返回需要从父契约归还的预算。
    ///
    /// 释放路径必须使用该值，而不是当前 `budget`：契约预算可能在预留后被修改，
    /// 直接按 `budget` 扣减会误伤兄弟契约的预留。
    pub fn take_reserved(&mut self) -> u64 {
        core::mem::take(&mut self.reserved)
    }

    /// 在父契约中预留本契约的预算；已预留时幂等。
    ///
    /// 用于带父链但未经 [`Self::split`] 创建的契约（例如 checkpoint 恢复）：
    /// 预留成功后父契约的 `children_budget` 与子契约的 `reserved` 同时更新。
    pub fn reserve_in_parent(&mut self, parent: &mut Self) -> Result<(), CadenceError> {
        if self.parent.is_none() || self.reserved != 0 {
            return Ok(());
        }
        let amount = self.budget.ticks();
        if amount > 0 && parent.children_budget.saturating_add(amount) > parent.budget.ticks() {
            return Err(CadenceError::BudgetExceedsParent);
        }
        parent.children_budget = parent.children_budget.saturating_add(amount);
        self.reserved = amount;
        Ok(())
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
    /// 调用方必须先通过 [`Self::take_reserved`] 归还父契约的预留。
    pub fn promote(&mut self) {
        self.parent = None;
        self.reserved = 0;
    }

    /// 递增 epoch，使旧句柄失效；返回新 epoch。
    pub fn revoke(&mut self) -> u32 {
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.epoch = 1;
        }
        self.epoch
    }

    /// 由内核把契约 epoch 与对象头 epoch 对齐。
    pub fn set_epoch(&mut self, epoch: u32) {
        self.epoch = epoch.max(1);
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
    #[allow(clippy::wildcard_imports)]
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
        let mut child = parent
            .split(CadenceId::new(2), weight(1024), Duration::from_ticks(4))
            .expect("子契约");
        assert_eq!(parent.children_budget(), 0, "未绑定不占用父预算");
        assert_eq!(child.reserved(), 0);
        child.reserve_in_parent(&mut parent).expect("绑定预留");
        assert_eq!(parent.children_budget(), 4);
        assert_eq!(child.reserved(), 4);
        parent.release_child_budget(child.take_reserved());
        assert_eq!(parent.children_budget(), 0);
        parent
            .split(CadenceId::new(3), weight(1024), Duration::from_ticks(7))
            .expect("释放后可重新分配");
    }

    #[test]
    fn admits_realtime_set_by_demand_bound() {
        assert!(edf_admissible(&[
            (3_000, 16_000, 8_000),
            (3_000, 16_000, 8_000)
        ]));
        assert!(!edf_admissible(&[
            (3_000, 16_000, 8_000),
            (3_000, 16_000, 8_000),
            (3_000, 16_000, 8_000),
        ]));

        // 隐式截止期：5 × 2ms / 10ms 恰好满载，6 个超载。
        assert!(edf_admissible(&[(2_000, 10_000, 0); 5]));
        assert!(!edf_admissible(&[(2_000, 10_000, 0); 6]));

        assert!(!edf_admissible(&[(0, 10_000, 0)]));
        assert!(!edf_admissible(&[(1_000, 0, 0)]));
    }

    #[test]
    fn splits_only_narrowing() {
        let mut parent = CadenceSpec::new(CadenceClass::Latency, 4096)
            .with_budget(Duration::from_ticks(8), Duration::from_ticks(32))
            .cadence(CadenceId::new(1))
            .expect("父契约");

        let mut child = parent
            .split(CadenceId::new(2), weight(1024), Duration::from_ticks(2))
            .expect("子契约");
        assert_eq!(child.parent(), Some(parent.id()));
        assert_eq!(child.epoch(), parent.epoch());
        assert_eq!(child.class(), parent.class());
        assert_eq!(parent.children_budget(), 0, "未绑定不占用父预算");
        assert_eq!(child.reserved(), 0);
        child.reserve_in_parent(&mut parent).expect("绑定预留");
        assert_eq!(parent.children_budget(), 2);
        assert_eq!(child.reserved(), 2);

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
    fn split_narrowed_checks_every_parent_bound() {
        let mut parent = CadenceSpec::new(CadenceClass::Realtime, 4096)
            .with_budget(Duration::from_ticks(4), Duration::from_ticks(16))
            .with_deadline(Duration::from_ticks(16))
            .with_latency(Duration::from_ticks(2))
            .with_affinity(CpuMask::from_bits(0b0011))
            .with_migratable(false)
            .cadence(CadenceId::new(1))
            .expect("父契约");
        let child_spec = CadenceSpec::new(CadenceClass::Realtime, 1024)
            .with_budget(Duration::from_ticks(1), Duration::from_ticks(16))
            .with_deadline(Duration::from_ticks(8))
            .with_latency(Duration::from_ticks(1))
            .with_affinity(CpuMask::single(0).expect("CPU 0"))
            .with_migratable(false)
            .with_lending(Lending::disabled());
        let child = parent
            .split_narrowed(CadenceId::new(2), child_spec)
            .expect("收窄子契约");
        assert_eq!(child.deadline(), Duration::from_ticks(8));
        assert_eq!(child.affinity(), CpuMask::single(0).expect("CPU 0"));
        assert!(!child.migratable());
        assert!(!child.lending().enabled());

        let mut affinity = child_spec;
        affinity.affinity = CpuMask::ALL;
        assert_eq!(
            parent.split_narrowed(CadenceId::new(3), affinity).err(),
            Some(CadenceError::AffinityNotSubset)
        );
        let mut period = child_spec;
        period.period = Duration::from_ticks(8);
        assert_eq!(
            parent.split_narrowed(CadenceId::new(4), period).err(),
            Some(CadenceError::PeriodNotNarrowing)
        );
        let mut deadline = child_spec;
        deadline.deadline = Duration::from_ticks(32);
        assert_eq!(
            parent.split_narrowed(CadenceId::new(5), deadline).err(),
            Some(CadenceError::DeadlineExceedsParent)
        );
        let mut latency = child_spec;
        latency.latency = Duration::from_ticks(3);
        assert_eq!(
            parent.split_narrowed(CadenceId::new(6), latency).err(),
            Some(CadenceError::LatencyExceedsParent)
        );
        let mut migratable = child_spec;
        migratable.migratable = true;
        assert_eq!(
            parent.split_narrowed(CadenceId::new(7), migratable).err(),
            Some(CadenceError::MigratableWidensParent)
        );
        let mut lending = child_spec;
        lending.lending = Lending::with_depth(3).expect("深度合法");
        assert_eq!(
            parent.split_narrowed(CadenceId::new(8), lending).err(),
            Some(CadenceError::LendingWidensParent)
        );
        let mut budget = child_spec;
        budget.budget = Duration::from_ticks(5);
        assert_eq!(
            parent.split_narrowed(CadenceId::new(9), budget).err(),
            Some(CadenceError::BudgetExceedsParent)
        );
    }

    #[test]
    fn restored_child_reserves_once_and_releases_exactly() {
        let mut parent = CadenceSpec::new(CadenceClass::Latency, 4096)
            .with_budget(Duration::from_ticks(8), Duration::from_ticks(32))
            .cadence(CadenceId::new(1))
            .expect("父契约");
        let spec = CadenceSpec::new(CadenceClass::Latency, 1024)
            .with_budget(Duration::from_ticks(4), Duration::from_ticks(32));

        // 模拟 checkpoint 恢复：带父链但未经 split，先预留再绑定。
        let mut restored =
            Cadence::restore(CadenceId::new(2), Some(parent.id()), 0, spec).expect("恢复契约");
        assert_eq!(restored.reserved(), 0);
        restored
            .reserve_in_parent(&mut parent)
            .expect("父契约有空间");
        assert_eq!(parent.children_budget(), 4);
        assert_eq!(restored.reserved(), 4);

        // 幂等；重复预留不得重复记账。
        restored
            .reserve_in_parent(&mut parent)
            .expect("重复预留幂等");
        assert_eq!(parent.children_budget(), 4);

        // 释放只按记录额度归还一次，不会误伤其他子契约。
        parent.release_child_budget(restored.take_reserved());
        assert_eq!(parent.children_budget(), 0);
        assert_eq!(restored.take_reserved(), 0);

        // 容量不足时拒绝，且不改变双方记账。
        parent.reserve_child_budget(7);
        let mut too_large = CadenceSpec::new(CadenceClass::Latency, 1024)
            .with_budget(Duration::from_ticks(2), Duration::from_ticks(32))
            .cadence(CadenceId::new(3))
            .expect("契约");
        too_large.parent = Some(parent.id());
        assert_eq!(
            too_large.reserve_in_parent(&mut parent),
            Err(CadenceError::BudgetExceedsParent)
        );
        assert_eq!(parent.children_budget(), 7);
        assert_eq!(too_large.reserved(), 0);
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
