//! 调度计量。

use hk_base::Duration;

use crate::{MeterSnapshot, Weight, charge_for};

/// 尚未在任一 CPU 上运行。
pub const NO_CPU: u32 = u32::MAX;

/// Task 的调度计量。
///
/// Meter 是调度的唯一计量来源；任务管理器、Debug Server 与 checkpoint 读取
/// 同一份数据。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Meter {
    consumed: u64,
    waited: u64,
    throttled: u64,
    overruns: u64,
    migrations: u64,
    switches: u64,
    virtual_pass: u64,
    last_cpu: u32,
}

impl Meter {
    /// 构造零计量。
    pub const fn new() -> Self {
        Self {
            consumed: 0,
            waited: 0,
            throttled: 0,
            overruns: 0,
            migrations: 0,
            switches: 0,
            virtual_pass: 0,
            last_cpu: NO_CPU,
        }
    }

    /// 记入实际执行时间，并推进虚拟时间。
    pub fn charge(&mut self, elapsed: Duration, weight: Weight) {
        self.consumed = self.consumed.saturating_add(elapsed.ticks());
        self.virtual_pass = self
            .virtual_pass
            .saturating_add(charge_for(elapsed, weight));
    }

    /// 记入在就绪队列中等待的时间。
    pub fn wait(&mut self, elapsed: Duration) {
        self.waited = self.waited.saturating_add(elapsed.ticks());
    }

    /// 记入一次预算耗尽。
    pub fn throttle(&mut self) {
        self.throttled = self.throttled.saturating_add(1);
    }

    /// 记入一次截止期或延迟目标超限。
    pub fn overrun(&mut self) {
        self.overruns = self.overruns.saturating_add(1);
    }

    /// 记入一次上下文切换。
    pub fn switch(&mut self) {
        self.switches = self.switches.saturating_add(1);
    }

    /// 记入一次 CPU 迁移；首次落位不计迁移。
    pub fn migrate(&mut self, cpu: u32) {
        if self.last_cpu != NO_CPU && self.last_cpu != cpu {
            self.migrations = self.migrations.saturating_add(1);
        }
        self.last_cpu = cpu;
    }

    /// 直接推进虚拟时间。
    pub fn advance_virtual(&mut self, amount: u64) {
        self.virtual_pass = self.virtual_pass.saturating_add(amount);
    }

    /// 清空全部计量。
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// 从快照还原计量。
    pub fn restore(snapshot: MeterSnapshot) -> Self {
        Self {
            consumed: snapshot.consumed,
            waited: snapshot.waited,
            throttled: snapshot.throttled,
            overruns: snapshot.overruns,
            migrations: snapshot.migrations,
            switches: snapshot.switches,
            virtual_pass: snapshot.virtual_pass,
            last_cpu: snapshot.last_cpu,
        }
    }

    /// 返回实际获得 CPU 的 tick 数。
    pub const fn consumed(&self) -> u64 {
        self.consumed
    }

    /// 返回等待 tick 数。
    pub const fn waited(&self) -> u64 {
        self.waited
    }

    /// 返回预算耗尽次数。
    pub const fn throttled(&self) -> u64 {
        self.throttled
    }

    /// 返回超限次数。
    pub const fn overruns(&self) -> u64 {
        self.overruns
    }

    /// 返回迁移次数。
    pub const fn migrations(&self) -> u64 {
        self.migrations
    }

    /// 返回上下文切换次数。
    pub const fn switches(&self) -> u64 {
        self.switches
    }

    /// 返回虚拟进度。
    pub const fn virtual_pass(&self) -> u64 {
        self.virtual_pass
    }

    /// 返回最近运行的 CPU。
    pub const fn last_cpu(&self) -> u32 {
        self.last_cpu
    }

    /// 生成快照。
    pub fn snapshot(&self) -> MeterSnapshot {
        MeterSnapshot::from_meter(self)
    }
}

impl Default for Meter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulates_consumption_and_virtual_time() {
        let mut meter = Meter::new();
        let weight = Weight::REFERENCE;
        meter.charge(Duration::from_ticks(10), weight);
        meter.wait(Duration::from_ticks(3));
        meter.switch();
        meter.throttle();
        meter.overrun();

        assert_eq!(meter.consumed(), 10);
        assert_eq!(meter.waited(), 3);
        assert_eq!(meter.virtual_pass(), 10);
        assert_eq!(meter.switches(), 1);
        assert_eq!(meter.throttled(), 1);
        assert_eq!(meter.overruns(), 1);
    }

    #[test]
    fn counts_migrations_after_first_placement() {
        let mut meter = Meter::new();
        meter.migrate(0);
        assert_eq!(meter.migrations(), 0);
        meter.migrate(1);
        assert_eq!(meter.migrations(), 1);
        meter.migrate(1);
        assert_eq!(meter.migrations(), 1);
    }

    #[test]
    fn round_trips_through_snapshot() {
        let mut meter = Meter::new();
        meter.charge(Duration::from_ticks(4), Weight::REFERENCE);
        meter.migrate(2);
        let restored = Meter::restore(meter.snapshot());
        assert_eq!(restored, meter);
    }
}
