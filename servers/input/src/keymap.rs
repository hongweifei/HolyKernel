//! evdev 常量与键码到字节序列的映射。

use hk_proto::input::{ansi, modified};

/// Linux evdev 事件类型：按键。
pub(crate) const EV_KEY: u16 = 1;
/// Linux evdev 事件类型：相对坐标。
pub(crate) const EV_REL: u16 = 2;
/// Linux evdev 事件类型：绝对坐标。
pub(crate) const EV_ABS: u16 = 3;
/// 绝对 X 轴。
pub(crate) const ABS_X: u16 = 0;
/// 绝对 Y 轴。
pub(crate) const ABS_Y: u16 = 1;
/// 滚轮轴。
pub(crate) const REL_WHEEL: u16 = 8;
/// 左 Shift。
pub(crate) const KEY_LEFTSHIFT: u16 = 42;
/// 右 Shift。
pub(crate) const KEY_RIGHTSHIFT: u16 = 54;
/// 左 Ctrl。
pub(crate) const KEY_LEFTCTRL: u16 = 29;
/// 右 Ctrl。
pub(crate) const KEY_RIGHTCTRL: u16 = 97;
/// 左 Alt。
pub(crate) const KEY_LEFTALT: u16 = 56;
/// 右 Alt。
pub(crate) const KEY_RIGHTALT: u16 = 100;
/// 鼠标左键。
pub(crate) const BTN_LEFT: u16 = 0x110;
/// 触摸/笔接触按钮。
pub(crate) const BTN_TOUCH: u16 = 0x14A;
/// 绝对坐标最大值。
pub(crate) const ABS_MAX: u32 = 0x7FFF;

/// 一次按键产生的字节序列。
pub(crate) struct KeyBytes {
    pub(crate) bytes: [u8; 8],
    pub(crate) len: usize,
}

/// 把 evdev 键码映射为待发送的字节序列。
pub(crate) fn key_bytes(code: u16, shift: bool, ctrl: bool, alt: bool) -> Option<KeyBytes> {
    if let Some(sequence) = special(code) {
        let mut bytes = [0_u8; 8];
        let len = sequence.len().min(bytes.len());
        bytes[..len].copy_from_slice(&sequence[..len]);
        return Some(KeyBytes { bytes, len });
    }

    let plain = printable(code)?;
    let (sequence, len) = modified(plain, shift, ctrl, alt);
    let mut bytes = [0_u8; 8];
    bytes[..len].copy_from_slice(&sequence[..len]);
    Some(KeyBytes { bytes, len })
}

/// 方向键与编辑键的 ANSI 序列。
fn special(code: u16) -> Option<&'static [u8]> {
    Some(match code {
        1 => ansi::ESCAPE,
        103 => ansi::UP,
        108 => ansi::DOWN,
        106 => ansi::RIGHT,
        105 => ansi::LEFT,
        102 => ansi::HOME,
        107 => ansi::END,
        104 => ansi::PAGE_UP,
        109 => ansi::PAGE_DOWN,
        110 => ansi::INSERT,
        111 => ansi::DELETE,
        _ => return None,
    })
}

/// 可打印键与基本控制键的 ASCII 映射。
fn printable(code: u16) -> Option<u8> {
    Some(match code {
        2..=11 => b'1' + (code as u8 - 2),
        12 => b'-',
        13 => b'=',
        14 => 0x08,
        15 => b'\t',
        16..=25 => b"qwertyuiop"[usize::from(code - 16)],
        26 => b'[',
        27 => b']',
        28 => b'\n',
        30..=38 => b"asdfghjkl"[usize::from(code - 30)],
        39 => b';',
        40 => b'\'',
        41 => b'`',
        43 => b'\\',
        44..=50 => b"zxcvbnm"[usize::from(code - 44)],
        51 => b',',
        52 => b'.',
        53 => b'/',
        57 => b' ',
        _ => return None,
    })
}
