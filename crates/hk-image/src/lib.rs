//! Holy 原生可执行镜像。
//!
//! 镜像是 Native Task 的清单：Task 记录描述入口、角色、优先级、栈与 Arena；
//! Region 记录描述内存区域；Capability 声明所需能力；Symbol 携带原生调试
//! 符号；Dependency 声明服务依赖。所有字段小端，记录区带 CRC32C 校验。
//!
//! 镜像构建只在宿主侧可用；内核与服务二进制只解析镜像和使用角色元数据。

#![no_std]

#[cfg(not(target_os = "none"))]
extern crate alloc;

#[cfg(not(target_os = "none"))]
mod builder;
mod format;
mod image;
mod role;
#[cfg(all(test, not(target_os = "none")))]
mod tests;
mod types;

#[cfg(not(target_os = "none"))]
pub use builder::build;
pub use format::{CAP_DEVICE, CAP_ENDPOINT, CAP_NOTIFICATION, FILE_SUFFIX, MAGIC, VERSION};
pub use image::Image;
pub use role::{SERVICE_ROLE_SECTION, ServiceRole};
pub use types::{
    Capability, CapabilityData, Dependency, DependencyData, Region, RegionData, Symbol, SymbolData,
    Task, TaskData,
};
