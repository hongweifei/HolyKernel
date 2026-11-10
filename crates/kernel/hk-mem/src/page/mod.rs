//! 早期物理页分配、页表页分配器与用户地址空间登记。

pub mod frame_bank;
pub mod pool;
pub mod registry;

pub use frame_bank::{FrameBank, FrameBankStats, FrameKind, LedgerStats};
pub use pool::PageAllocator;
pub use registry::{AddressSpaceRegistry, UserAddressSpace};
