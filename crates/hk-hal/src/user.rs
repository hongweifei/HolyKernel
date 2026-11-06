//! 用户虚拟地址布局。
//!
//! 每个任务有独立的地址空间；镜像、栈与 Arena 由平台布局固定，动态映射
//! （Frame、DMA、MMIO 与帧缓冲）统一从自动窗口分配

/// 服务镜像基址；与构建期 `hk-server-build::IMAGE_BASE` 一致。
pub const IMAGE_BASE: u64 = 0x1000_0000;
/// 服务镜像区结束地址（不含）。
pub const IMAGE_LIMIT: u64 = 0x1100_0000;
/// 任务 Arena 基址。
pub const ARENA_BASE: u64 = 0x0200_0000;
/// 单个任务的 Arena 大小。
pub const ARENA_SIZE: u64 = 2 * 1024 * 1024;
/// 相邻任务 Arena 的间隔。
pub const ARENA_STRIDE: u64 = ARENA_SIZE;
/// 任务用户栈基址。
pub const STACK_BASE: u64 = 0x0400_0000;
/// 用户栈大小。
pub const STACK_SIZE: u64 = 512 * 1024;
/// 相邻任务用户栈的间隔。
pub const STACK_STRIDE: u64 = 2 * 1024 * 1024;

/// 自动映射窗口起始地址。
pub const AUTO_WINDOW_BASE: u64 = 0x2000_0000;
/// 自动映射窗口结束地址（不含），保持在 2 GiB 以内。
pub const AUTO_WINDOW_LIMIT: u64 = 0x8000_0000;

const _: () = {
    assert!(ARENA_BASE.checked_add(ARENA_SIZE).unwrap() <= STACK_BASE);
    assert!(STACK_BASE.checked_add(STACK_STRIDE).unwrap() <= IMAGE_BASE);
    assert!(IMAGE_LIMIT <= AUTO_WINDOW_BASE);
};
