//! Holy Cadence Model 的共享契约。
//!
//! 本 crate 定义 Cadence 与 Meter 的值类型、调度类别、权重与虚拟时间数学、
//! 亲和性与借贷策略，以及 checkpoint 使用的固定长度快照编码。内核 Beat、
//! Tempo Server、Root Server 与任务管理器共享同一份定义。
//!
//! 这里只有契约，不含对象存储、队列或策略：Cadence 的授予、拆分与撤销由
//! Tempo Server 决定，内核只执行生效契约。

#![no_std]

mod affinity;
mod cadence;
mod class;
mod error;
mod lending;
mod meter;
mod pool;
mod snapshot;
mod time;
mod weight;

pub use affinity::{CpuMask, MAX_CPUS};
pub use cadence::{
    ADMISSION_HORIZON_TICKS, Cadence, CadenceId, CadenceSpec, DEFAULT_SPEC, IDLE_SPEC,
    UTILIZATION_SCALE, admits_realtime, density, edf_admissible, utilization,
};
pub use class::CadenceClass;
pub use error::CadenceError;
pub use lending::{DEFAULT_LENDING_DEPTH, Lending, MAX_LENDING_DEPTH};
pub use meter::{Meter, NO_CPU};
pub use pool::{BudgetLease, BudgetPool, BudgetPoolId};
pub use snapshot::{CadenceSnapshot, MeterSnapshot};
pub use time::{TIME_BASE_HZ, from_platform, micros, millis, to_platform};
pub use weight::{
    BASE_SLICE, MAX_SLICE, MAX_WEIGHT, MIGRATION_BASE_COST, MIN_SLICE, WEIGHT_REFERENCE, Weight,
    charge_for, migration_cost, slice_for,
};
