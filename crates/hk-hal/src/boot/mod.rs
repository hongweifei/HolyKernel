//! 平台无关的启动信息。
//!
//! 内存 map、内核镜像、帧缓冲、direct map、DTB/RSDP 与服务器模块由
//! bootloader 以固定布局交给内核；本模块负责描述与校验。

mod display;
#[cfg(test)]
mod tests;
mod types;
mod validate;

pub use types::{
    BootInfo, BootModule, DirectMapInfo, FramebufferInfo, MemoryRegion, PixelFormat, RegionKind,
};
pub use validate::BootInfoError;

/// BootInfo 魔数。
pub const BOOT_INFO_MAGIC: u64 = 0x484B_424F_4F54_3031;

/// 当前 BootInfo 版本。
pub const BOOT_INFO_VERSION: u32 = 1;
