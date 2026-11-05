//! 输入设备事件协议。
//!
//! 用户态输入驱动把已解码的按键字节发给显示服务，由显示服务按焦点路由到
//! 窗口客户端；内核控制台输入在输入驱动模式下关闭。

/// 按键事件；`word` 为字节值。
pub const INPUT_KEY: u64 = 0xB00;
/// 绝对指针位置；`word` 为打包的 16 位 x/y（0..=0x7FFF）。
pub const INPUT_POINTER: u64 = 0xB01;
/// 指针按钮；`word` 非 0 为按下。
pub const INPUT_BUTTON: u64 = 0xB02;

/// 打包绝对指针坐标。
pub const fn pack_pointer(x: u16, y: u16) -> u64 {
    ((x as u64) << 16) | y as u64
}

/// 解包绝对指针坐标。
pub const fn unpack_pointer(word: u64) -> (u16, u16) {
    ((word >> 16) as u16, word as u16)
}
