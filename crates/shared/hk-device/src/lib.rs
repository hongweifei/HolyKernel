//! 设备描述与 PCI 枚举。
//!
//! 内核、平台、驱动与用户态服务共用：`DeviceGrant` 描述授权给驱动的资源，
//! `pci` 定义配置空间地址与访问契约，`scan` 在其上做通用总线枚举。
//! 本 crate 不依赖平台 trait，只使用 `hk-base` 的共享类型。

#![no_std]

#[cfg(test)]
extern crate alloc;

pub mod device;
pub mod grant;
pub mod pci;
pub mod scan;

pub use device::{DEVICE_GRANT_BYTES, DeviceGrant, IoRange, MmioRange};
pub use pci::{EcamPci, PciAddress, PciConfig};
