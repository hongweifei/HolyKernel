//! 外部驱动 crate 的宿主适配：DMA、MMIO 与日志。
//!
//! `ahci-driver`、`nvme-driver`、`crab-usb` 都通过 `dma-api`、`mmio-api` 和
//! `log` 与宿主交互；本 crate 把这三者接到 HolyKernel 的 DeviceGrant、能力
//! DMA、设备页映射与控制台。

#![cfg_attr(target_os = "none", no_std)]

extern crate alloc;

pub mod dma;
pub mod log;
pub mod mmio;

pub use dma::{init as init_dma, register_mapping};
pub use log::init as init_log;
pub use mmio::{init as init_mmio, op as mmio_op, register_mmio};
