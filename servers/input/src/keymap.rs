//! evdev 常量与键码到字节的映射。

/// Linux evdev 事件类型：按键。
#[cfg(target_os = "none")]
pub(crate) const EV_KEY: u16 = 1;
/// Linux evdev 事件类型：绝对坐标。
#[cfg(target_os = "none")]
pub(crate) const EV_ABS: u16 = 3;
/// 绝对 X 轴。
#[cfg(target_os = "none")]
pub(crate) const ABS_X: u16 = 0;
/// 绝对 Y 轴。
#[cfg(target_os = "none")]
pub(crate) const ABS_Y: u16 = 1;
/// 左 Shift。
#[cfg(target_os = "none")]
pub(crate) const KEY_LEFTSHIFT: u16 = 42;
/// 右 Shift。
#[cfg(target_os = "none")]
pub(crate) const KEY_RIGHTSHIFT: u16 = 54;
/// 鼠标左键。
#[cfg(target_os = "none")]
pub(crate) const BTN_LEFT: u16 = 0x110;
/// 触摸/笔接触按钮。
#[cfg(target_os = "none")]
pub(crate) const BTN_TOUCH: u16 = 0x14A;
/// 绝对坐标最大值。
#[cfg(target_os = "none")]
pub(crate) const ABS_MAX: u32 = 0x7FFF;

/// 把 evdev 键码映射为字节。
#[cfg(target_os = "none")]
pub(crate) fn key_byte(code: u16, shift: bool) -> Option<u8> {
    let plain = match code {
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
    };
    Some(if shift { shifted(plain) } else { plain })
}

/// 应用 Shift 映射。
#[cfg(target_os = "none")]
fn shifted(byte: u8) -> u8 {
    match byte {
        b'a'..=b'z' => byte - 32,
        b'1' => b'!',
        b'2' => b'@',
        b'3' => b'#',
        b'4' => b'$',
        b'5' => b'%',
        b'6' => b'^',
        b'7' => b'&',
        b'8' => b'*',
        b'9' => b'(',
        b'0' => b')',
        b'-' => b'_',
        b'=' => b'+',
        b'[' => b'{',
        b']' => b'}',
        b';' => b':',
        b'\'' => b'"',
        b'`' => b'~',
        b'\\' => b'|',
        b',' => b'<',
        b'.' => b'>',
        b'/' => b'?',
        other => other,
    }
}
