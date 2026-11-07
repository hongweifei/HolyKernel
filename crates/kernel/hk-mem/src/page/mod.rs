//! 早期物理页分配、页表页池与用户地址空间登记。

pub mod linear;
pub mod pool;
pub mod registry;

pub use linear::LinearPages;
pub use pool::PagePool;
pub use registry::{UserVspace, VspaceRegistry};
