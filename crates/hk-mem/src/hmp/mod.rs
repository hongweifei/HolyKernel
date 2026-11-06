//! Holy Memory Plane（HMP）核心对象模型。
//!
//! 本模块定义 Region 统一映射、类型化能力、Provider 契约与 Ledger 记账的
//! 对象层。内核对象、系统调用与服务接入在后续阶段完成。

pub mod backing;
pub mod capability;
pub mod capacity;
pub mod error;
pub mod frame;
pub mod ledger;
pub mod provider;
pub mod region;

pub use backing::{Backing, set_backing_releaser};
pub use capability::{Capability, CapabilityKind, CapabilityOp, Mappable};
pub use capacity::{Capacity, CapacityId, CapacityKind};
pub use error::HmpError;
pub use frame::{Frame, FrameId};
pub use ledger::{Charge, Ledger, LedgerId};
pub use provider::{FaultReply, FaultRequest, ProviderId, ProviderKind};
pub use region::{Region, RegionFlags, RegionId};
