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
/// 键盘事件；`word` 为打包的窗口编号与字节，发给焦点窗口的客户端。
pub const DISPLAY_KEY: u64 = 0xA05;
/// 设置窗口标题；载荷为 8 字节窗口编号 + UTF-8 标题。
pub const DISPLAY_SET_TITLE: u64 = 0xA06;
/// 窗口被鼠标点击；`word` 为窗口编号。
pub const DISPLAY_CLICK: u64 = 0xA07;
/// 窗口被关闭；`word` 为窗口编号。
pub const DISPLAY_CLOSE: u64 = 0xA08;
/// 窗口正文点击位置；`word` 打包窗口编号与窗口内坐标。
pub const DISPLAY_CLICK_AT: u64 = 0xA09;

/// 窗口标题最大字节数。
pub const TITLE_MAX: usize = 32;

/// 设置窗口标题（非阻塞，显示服务下一轮处理）。
pub fn set_title(display: u64, window: u64, title: &str) {
    let mut buffer = [0_u8; 8 + TITLE_MAX];
    buffer[..8].copy_from_slice(&window.to_le_bytes());
    let length = title.len().min(TITLE_MAX);
    buffer[8..8 + length].copy_from_slice(&title.as_bytes()[..length]);
    let _ = crate::send_data(display, DISPLAY_SET_TITLE, &buffer[..8 + length]);
}

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

/// 把窗口编号与窗口内坐标打包进一个消息字。
pub const fn pack_click_at(window: u64, x: u32, y: u32) -> u64 {
    (window << 32) | ((x as u64 & 0xFFFF) << 16) | (y as u64 & 0xFFFF)
}

/// 从消息字解包窗口编号与窗口内坐标。
pub const fn unpack_click_at(word: u64) -> (u64, u32, u32) {
    (
        word >> 32,
        ((word >> 16) & 0xFFFF) as u32,
        (word & 0xFFFF) as u32,
    )
}
