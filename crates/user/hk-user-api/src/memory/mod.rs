//! HMP 类型化能力与 Region 映射的系统调用封装。
//!
//! 能力与 Capacity 在 `capability`，Frame 与对象枚举在 `frame`，Region、Arena
//! 与 DMA 映射在 `region`，Provider 协议常量与审计/账本查询在 `provider`；
//! 全部条目在此重新导出，保持 `crate::memory::*` 的原有路径。

mod capability;
mod frame;
mod provider;
mod region;

pub use capability::*;
pub use frame::*;
pub use provider::*;
pub use region::*;
