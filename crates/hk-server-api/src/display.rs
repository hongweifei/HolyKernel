//! 显示服务协议：窗口创建、表面提交与帧完成。
//!
//! 客户端分配共享 Frame 作为窗口表面，绘制后把句柄随创建请求交给显示服务；
//! 显示服务把表面映射进自己的地址空间并合成到帧缓冲，不需要复制整块像素。

pub use hk_syscall::DISPLAY_SERVICE;

/// 创建窗口；`word` 打包宽高，随消息转移表面 Frame 句柄。
pub const DISPLAY_CREATE_WINDOW: u64 = 0xA00;
/// 窗口已创建；`word` 为窗口编号。
pub const DISPLAY_WINDOW_CREATED: u64 = 0xA01;
/// 提交窗口表面；`word` 为窗口编号。
pub const DISPLAY_COMMIT: u64 = 0xA02;
/// 一帧已合成；`word` 为窗口编号。
pub const DISPLAY_FRAME_DONE: u64 = 0xA03;
/// 请求显示服务退出（用于自检收尾）。
pub const DISPLAY_SHUTDOWN: u64 = 0xA04;
/// 键盘事件；`word` 为字节值，发给焦点窗口的客户端。
pub const DISPLAY_KEY: u64 = 0xA05;

/// 客户端映射窗口表面使用的用户虚拟地址。
pub const SURFACE_VIRT: u64 = 0x5000_0000;

/// 把宽高打包进一个消息字。
pub const fn pack_size(width: u32, height: u32) -> u64 {
    ((width as u64) << 32) | height as u64
}

/// 从消息字解包宽高。
pub const fn unpack_size(word: u64) -> (u32, u32) {
    ((word >> 32) as u32, word as u32)
}

/// 把窗口编号与按键字节打包进一个消息字。
pub const fn pack_key(window: u64, byte: u8) -> u64 {
    (window << 32) | byte as u64
}

/// 从消息字解包窗口编号与按键字节。
pub const fn unpack_key(word: u64) -> (u64, u8) {
    (word >> 32, word as u8)
}
