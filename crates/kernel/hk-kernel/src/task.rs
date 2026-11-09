//! Native Task：调度主体。
//!
//! Task 持有 Cadence 与 Meter，TCB 是 Task 内的执行单元。公平、预算与计量都
//! 记在 Task 上；多个 TCB 共享同一份契约与计量。

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use hk_base::Duration;
use hk_cadence::{Cadence, CadenceClass, Meter, NO_CPU, slice_for};
use spin::Mutex;

use crate::tcb::TcbId;

static NEXT_TASK_ID: AtomicU64 = AtomicU64::new(1);

/// Task 标识。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskId(u64);

impl TaskId {
    /// 以数值构造标识。
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// 分配一个内核内唯一标识。
    pub fn next() -> Self {
        Self(NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed))
    }

    /// 返回标识数值。
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Task 调度状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskState {
    /// 已创建，尚未附加契约。
    Created,
    /// 可运行。
    Eligible,
    /// 正在 CPU 上执行。
    Running,
    /// 全部 TCB 阻塞。
    Blocked,
    /// 预算耗尽，等待周期补充。
    Throttled,
    /// 被 Root/Tempo 挂起。
    Suspended,
    /// 已退出。
    Exited,
}

/// Task 调度状态的只读快照。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskMetrics {
    /// 调度类别。
    pub class: CadenceClass,
    /// 权重。
    pub weight: u32,
    /// 实际执行时间（契约 tick）。
    pub consumed: u64,
    /// 虚拟进度。
    pub virtual_pass: u64,
    /// 预算耗尽次数。
    pub throttled: u64,
}

/// 调度主体。
pub struct Task {
    id: TaskId,
    parent: Option<TaskId>,
    cadence: Arc<Mutex<Cadence>>,
    meter: Arc<Mutex<Meter>>,
    threads: Vec<TcbId>,
    state: TaskState,
    policy_authority: bool,
    period_start: u64,
    budget_left: u64,
    grant_end: u64,
}

impl Task {
    /// 以契约与计量创建 Task。
    pub fn new(id: TaskId, parent: Option<TaskId>, cadence: Cadence, meter: Meter) -> Self {
        let budget_left = cadence.budget().ticks();
        Self {
            id,
            parent,
            cadence: Arc::new(Mutex::new(cadence)),
            meter: Arc::new(Mutex::new(meter)),
            threads: Vec::new(),
            state: TaskState::Created,
            policy_authority: false,
            period_start: 0,
            budget_left,
            grant_end: 0,
        }
    }

    /// 返回 Task 标识。
    pub const fn id(&self) -> TaskId {
        self.id
    }

    /// 返回父 Task。
    pub const fn parent(&self) -> Option<TaskId> {
        self.parent
    }

    /// 返回 Cadence 契约。
    pub fn cadence(&self) -> Arc<Mutex<Cadence>> {
        Arc::clone(&self.cadence)
    }

    /// 返回 Meter 计量。
    pub fn meter(&self) -> Arc<Mutex<Meter>> {
        Arc::clone(&self.meter)
    }

    /// 附加一个 TCB。
    pub fn attach(&mut self, tcb: TcbId) {
        if !self.threads.contains(&tcb) {
            self.threads.push(tcb);
        }
    }

    /// 分离一个 TCB；返回 Task 是否已无线程。
    pub fn detach(&mut self, tcb: TcbId) -> bool {
        self.threads.retain(|thread| *thread != tcb);
        self.threads.is_empty()
    }

    /// 返回全部 TCB。
    pub fn threads(&self) -> &[TcbId] {
        &self.threads
    }

    /// 返回 Task 状态。
    pub const fn state(&self) -> TaskState {
        self.state
    }

    /// 设置 Task 状态。
    pub fn set_state(&mut self, state: TaskState) {
        self.state = state;
    }

    /// 判断 Task 是否持有调度策略授权。
    pub const fn policy_authority(&self) -> bool {
        self.policy_authority
    }

    /// 设置调度策略授权；由 Root 在启动时授予 Tempo。
    pub fn set_policy_authority(&mut self, policy_authority: bool) {
        self.policy_authority = policy_authority;
    }

    /// 替换 Task 的 Cadence，并重置预算周期。
    pub fn set_cadence(&mut self, cadence: Arc<Mutex<Cadence>>) {
        self.cadence = cadence;
        self.period_start = 0;
        self.budget_left = self.cadence.lock().budget().ticks();
        if matches!(self.state, TaskState::Suspended | TaskState::Created) {
            self.state = TaskState::Eligible;
        }
    }

    /// 返回类别。
    pub fn class(&self) -> CadenceClass {
        self.cadence.lock().class()
    }

    /// 返回当前时间片长度（契约 tick）。
    pub fn slice_ticks(&self) -> u64 {
        slice_for(self.cadence.lock().weight()).ticks()
    }

    /// 返回只读计量快照。
    pub fn metrics(&self) -> TaskMetrics {
        let cadence = self.cadence();
        let cadence = cadence.lock();
        let meter = self.meter();
        let meter = meter.lock();
        TaskMetrics {
            class: cadence.class(),
            weight: cadence.weight().get(),
            consumed: meter.consumed(),
            virtual_pass: meter.virtual_pass(),
            throttled: meter.throttled(),
        }
    }

    /// 返回当前 Grant 的结束时刻。
    pub const fn grant_end(&self) -> u64 {
        self.grant_end
    }

    /// 设置当前 Grant 的结束时刻。
    pub fn set_grant_end(&mut self, end: u64) {
        self.grant_end = end;
    }

    /// 返回本周期剩余预算（契约 tick）。
    pub const fn budget_left(&self) -> u64 {
        self.budget_left
    }

    /// 判断契约是否带硬预算。
    pub fn has_budget(&self) -> bool {
        !self.cadence.lock().budget().is_zero()
    }

    /// 判断预算是否耗尽。
    pub fn budget_exhausted(&self) -> bool {
        self.has_budget() && self.budget_left == 0
    }

    /// 计算队列键：实时类按绝对截止期，低延迟类按虚拟截止期，吞吐类按虚拟进度。
    pub fn queue_key(&self) -> u64 {
        let cadence = self.cadence.lock();
        let meter = self.meter.lock();
        match cadence.class() {
            CadenceClass::Realtime => {
                let deadline = if cadence.deadline().is_zero() {
                    cadence.period().ticks()
                } else {
                    cadence.deadline().ticks()
                };
                self.period_start.saturating_add(deadline)
            }
            CadenceClass::Latency => meter
                .virtual_pass()
                .saturating_add(cadence.latency().ticks()),
            CadenceClass::Throughput => meter.virtual_pass(),
            CadenceClass::Idle => 0,
        }
    }

    /// 记入一次执行：更新 Meter 与剩余预算。
    pub fn charge(&mut self, elapsed: u64) {
        let (weight, budget) = {
            let cadence = self.cadence.lock();
            (cadence.weight(), cadence.budget().ticks())
        };
        self.meter
            .lock()
            .charge(Duration::from_ticks(elapsed), weight);
        if budget > 0 {
            self.budget_left = self
                .budget_left
                .saturating_sub(elapsed.min(self.budget_left));
        }
    }

    /// 记入借出的执行时间；预算耗尽时进入 `Throttled`。
    pub fn charge_borrowed(&mut self, elapsed: u64) {
        self.charge(elapsed);
        if self.budget_exhausted() && self.state != TaskState::Throttled {
            self.throttle();
        }
    }

    /// 返回借贷深度上限；未启用借贷时为 0。
    pub fn lending_limit(&self) -> u64 {
        let cadence = self.cadence.lock();
        let lending = cadence.lending();
        if lending.enabled() {
            u64::from(lending.max_depth())
        } else {
            0
        }
    }

    /// 判断 Task 是否允许在 `cpu` 上运行。
    pub fn cpu_allowed(&self, cpu: usize) -> bool {
        let cpu = cpu as u32;
        let cadence = self.cadence.lock();
        if !cadence.affinity().contains(cpu) {
            return false;
        }
        if cadence.migratable() {
            return true;
        }
        let last = self.meter.lock().last_cpu();
        last == NO_CPU || last == cpu
    }

    /// 返回亲和性位图。
    pub fn affinity_bits(&self) -> u64 {
        self.cadence.lock().affinity().bits()
    }

    /// 判断是否参与迁移。
    pub fn migratable(&self) -> bool {
        self.cadence.lock().migratable()
    }

    /// 返回调度权重。
    pub fn weight(&self) -> u32 {
        self.cadence.lock().weight().get()
    }

    /// 返回最近运行的 CPU；尚未运行时为 `NO_CPU`。
    pub fn last_cpu(&self) -> u32 {
        self.meter.lock().last_cpu()
    }

    /// 记录 Task 本次运行的 CPU，并累计迁移次数。
    pub fn record_cpu(&mut self, cpu: usize) {
        self.meter.lock().migrate(cpu as u32);
    }

    /// 周期到期时补充预算；返回是否发生补充。
    pub fn replenish(&mut self, now: u64) -> bool {
        let period = self.cadence.lock().period().ticks();
        if period == 0 {
            return false;
        }
        let refill = self.period_start.saturating_add(period);
        if now < refill {
            return false;
        }
        self.period_start = now;
        self.budget_left = self.cadence.lock().budget().ticks();
        if self.state == TaskState::Throttled {
            self.state = TaskState::Eligible;
        }
        true
    }

    /// 返回下一周期补充时刻；未处于 Throttled 时返回 `None`。
    pub fn refill_at(&self) -> Option<u64> {
        if self.state != TaskState::Throttled {
            return None;
        }
        let period = self.cadence.lock().period().ticks();
        if period == 0 {
            return None;
        }
        Some(self.period_start.saturating_add(period))
    }

    /// 记入一次超限。
    pub fn overrun(&mut self) {
        self.meter.lock().overrun();
    }

    /// 记入一次预算耗尽。
    pub fn throttle(&mut self) {
        self.meter.lock().throttle();
        self.set_state(TaskState::Throttled);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hk_cadence::{CadenceId, CadenceSpec, Weight, millis};

    fn task(spec: CadenceSpec) -> Task {
        let cadence = spec.cadence(CadenceId::new(1)).expect("契约合法");
        Task::new(TaskId::new(1), None, cadence, Meter::new())
    }

    #[test]
    fn attaches_and_detaches_threads() {
        let mut task = task(CadenceSpec::new(CadenceClass::Latency, 1024));
        task.attach(TcbId::new(1));
        task.attach(TcbId::new(2));
        task.attach(TcbId::new(1));
        assert_eq!(task.threads(), &[TcbId::new(1), TcbId::new(2)]);
        assert!(!task.detach(TcbId::new(1)));
        assert!(task.detach(TcbId::new(2)));
    }

    #[test]
    fn charges_budget_and_refills_periodically() {
        let spec = CadenceSpec::new(CadenceClass::Realtime, 1024)
            .with_budget(millis(3), millis(16))
            .with_deadline(millis(8));
        let mut task = task(spec);
        assert!(task.has_budget());
        assert_eq!(task.budget_left(), 3_000);
        task.charge(2_000);
        assert_eq!(task.budget_left(), 1_000);
        task.charge(2_000);
        assert_eq!(task.budget_left(), 0);
        assert!(task.budget_exhausted());
        task.throttle();
        assert_eq!(task.state(), TaskState::Throttled);
        assert_eq!(task.refill_at(), Some(16_000));

        assert!(task.replenish(16_000));
        assert_eq!(task.state(), TaskState::Eligible);
        assert_eq!(task.budget_left(), 3_000);
    }

    #[test]
    fn computes_class_specific_queue_keys() {
        let throughput = task(CadenceSpec::new(CadenceClass::Throughput, 1024));
        assert_eq!(throughput.queue_key(), 0);

        let realtime = task(
            CadenceSpec::new(CadenceClass::Realtime, 1024)
                .with_budget(millis(1), millis(4))
                .with_deadline(millis(2)),
        );
        assert_eq!(realtime.queue_key(), 2_000);

        let latency = task(CadenceSpec::new(CadenceClass::Latency, 1024).with_latency(millis(1)));
        assert_eq!(latency.queue_key(), 1_000);
    }

    #[test]
    fn virtual_pass_advances_with_weight() {
        let mut task = task(CadenceSpec::new(CadenceClass::Throughput, 2048));
        task.charge(10);
        assert_eq!(task.meter().lock().virtual_pass(), 5);
        assert_eq!(Weight::REFERENCE.get(), 1024);
    }
}
