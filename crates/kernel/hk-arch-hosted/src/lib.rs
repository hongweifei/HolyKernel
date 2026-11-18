//! HolyKernel Hosted 模拟器平台。
//!
//! Hosted 平台把微内核与用户态服务器作为普通进程运行：虚拟地址空间、虚拟时钟、
//! 模拟设备与故障注入都在进程内实现，用于单元测试、CI 与确定性回放。

pub mod boot;
pub mod console;
pub mod dma;
pub mod entropy;
pub mod irq;
pub mod physical;
pub mod platform;
pub mod timer;

pub use boot::{HOSTED_RAM_BASE, HOSTED_RAM_SIZE, boot_info};
pub use config::HostedConfig;
pub use platform::HostedPlatform;

mod config;
