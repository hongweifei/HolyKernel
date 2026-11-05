//! evdev 常量与键码到字节序列的映射。

/// Linux evdev 事件类型：按键。
#[cfg(target_os = "none")]
pub(crate) const EV_KEY: u16 = 1;
/// Linux evdev 事件类型：相对坐标。
#[cfg(target_os = "none")]
pub(crate) const EV_REL: u16 = 2;
/// Linux evdev 事件类型：绝对坐标。
#[cfg(target_os = "none")]
pub(crate) const EV_ABS: u16 = 3;
/// 绝对 X 轴。
#[cfg(target_os = "none")]
pub(crate) const ABS_X: u16 = 0;
/// 绝对 Y 轴。
#[cfg(target_os = "none")]
pub(crate) const ABS_Y: u16 = 1;
/// 滚轮轴。
#[cfg(target_os = "none")]
pub(crate) const REL_WHEEL: u16 = 8;
/// 左 Shift。
#[cfg(target_os = "none")]
pub(crate) const KEY_LEFTSHIFT: u16 = 42;
/// 右 Shift。
#[cfg(target_os = "none")]
pub(crate) const KEY_RIGHTSHIFT: u16 = 54;
/// 左 Ctrl。
#[cfg(target_os = "none")]
pub(crate) const KEY_LEFTCTRL: u16 = 29;
/// 右 Ctrl。
#[cfg(target_os = "none")]
pub(crate) const KEY_RIGHTCTRL: u16 = 97;
/// 左 Alt。
#[cfg(target_os = "none")]
pub(crate) const KEY_LEFTALT: u16 = 56;
/// 右 Alt。
#[cfg(target_os = "none")]
pub(crate) const KEY_RIGHTALT: u16 = 100;
/// 鼠标左键。
#[cfg(target_os = "none")]
pub(crate) const BTN_LEFT: u16 = 0x110;
/// 触摸/笔接触按钮。
#[cfg(target_os = "none")]
pub(crate) const BTN_TOUCH: u16 = 0x14A;
/// 绝对坐标最大值。
#[cfg(target_os = "none")]
pub(crate) const ABS_MAX: u32 = 0x7FFF;

/// 一次按键产生的字节序列。
#[cfg(target_os = "none")]
pub(crate) struct KeyBytes {
    pub(crate) bytes: [u8; 8],
    pub(crate) len: usize,
}

/// 把 evdev 键码映射为待发送的字节序列。
#[cfg(target_os = "none")]
pub(crate) fn key_bytes(code: u16, shift: bool, ctrl: bool, alt: bool) -> Option<KeyBytes> {
    if let Some(sequence) = special(code) {
        let mut bytes = [0_u8; 8];
        let len = sequence.len().min(bytes.len());
        bytes[..len].copy_from_slice(&sequence[..len]);
        return Some(KeyBytes { bytes, len });
    }

    let plain = printable(code)?;
    let byte = if shift { shifted(plain) } else { plain };
    let byte = if ctrl {
        control(byte).unwrap_or(byte)
    } else {
        byte
    };
    let mut bytes = [0_u8; 8];
    let mut len = 0;
    if alt {
        bytes[len] = 0x1B;
        len += 1;
    }
    bytes[len] = byte;
    Some(KeyBytes {
        bytes,
        len: len + 1,
    })
}

/// 方向键与编辑键的 ANSI 序列。
#[cfg(target_os = "none")]
fn special(code: u16) -> Option<&'static [u8]> {
    Some(match code {
        1 => &b"\x1b"[..],
        103 => &b"\x1b[A"[..],
        108 => &b"\x1b[B"[..],
        106 => &b"\x1b[C"[..],
        105 => &b"\x1b[D"[..],
        102 => &b"\x1b[H"[..],
        107 => &b"\x1b[F"[..],
        104 => &b"\x1b[5~"[..],
        109 => &b"\x1b[6~"[..],
        110 => &b"\x1b[2~"[..],
        111 => &b"\x1b[3~"[..],
        _ => return None,
    })
}

/// 可打印键与基本控制键的 ASCII 映射。
#[cfg(target_os = "none")]
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

/// 应用 Ctrl 映射。
#[cfg(target_os = "none")]
fn control(byte: u8) -> Option<u8> {
    Some(match byte {
        b'a'..=b'z' => byte - b'a' + 1,
        b'A'..=b'Z' => byte - b'A' + 1,
        b'@' | b' ' => 0,
        b'[' => 0x1B,
        b'\\' => 0x1C,
        b']' => 0x1D,
        b'^' => 0x1E,
        b'_' => 0x1F,
        _ => return None,
    })
}
