//! Holy Memory Plane（HMP）核心对象模型。
//!
//! 本模块定义 Region 统一映射、类型化能力、Provider 契约与 Ledger 记账的
//! 对象层；对象由内核句柄承载，运行时经 `SYS_*` 系统调用与服务接入使用。

pub mod arena;
pub mod backend;
pub mod capability;
pub mod capacity;
pub mod error;
pub mod frame;
pub mod frame_ledger;
pub mod lease;
pub mod ledger;
pub mod owned;
pub mod provider;
pub mod region;

pub use arena::{Arena, ArenaId, ArenaRegion};
pub use backend::{PhysicalBackend, physical_backend, set_physical_backend};
pub use capability::{Capability, CapabilityKind, CapabilityOp, Mappable};
pub use capacity::{Capacity, CapacityId, CapacityKind, set_frame_scrubber};
pub use error::HmpError;
pub use frame::{Frame, FrameId};
pub use frame_ledger::{FrameLedger, FrameRef};
pub use lease::FrameLease;
pub use ledger::{Charge, Ledger, LedgerId, LedgerSnapshot, SharedLedger};
pub use owned::{FrameBacking, OwnedFrame};
pub use provider::{FaultReply, FaultRequest, ProviderKind};
pub use region::{Region, RegionFlags, RegionId};
