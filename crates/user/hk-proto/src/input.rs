//! 输入设备事件协议。
//!
//! 用户态输入驱动把按键字节序列、指针位置与滚轮增量发给显示服务，由显示
//! 服务按焦点路由到窗口客户端；特殊键使用 ANSI 序列，消费端用 [`Decoder`]
//! 还原。内核控制台输入在输入驱动模式下关闭。

/// 按键事件；`word` 为字节值。
pub const INPUT_KEY: u64 = crate::labels::INPUT;
/// 绝对指针位置；`word` 为打包的 16 位 x/y（0..=0x7FFF）。
pub const INPUT_POINTER: u64 = crate::labels::INPUT + 1;
/// 指针按钮；`word` 非 0 为按下。
pub const INPUT_BUTTON: u64 = crate::labels::INPUT + 2;
/// 鼠标滚轮；`word` 为有符号增量，正值向上。
pub const INPUT_WHEEL: u64 = crate::labels::INPUT + 3;

/// 打包滚轮增量。
pub const fn pack_wheel(delta: i32) -> u64 {
    delta as u32 as u64
}

/// 解包滚轮增量。
pub const fn unpack_wheel(word: u64) -> i32 {
    word as u32 as i32
}

/// 打包绝对指针坐标。
pub const fn pack_pointer(x: u16, y: u16) -> u64 {
    ((x as u64) << 16) | y as u64
}

/// 解包绝对指针坐标。
pub const fn unpack_pointer(word: u64) -> (u16, u16) {
    ((word >> 16) as u16, word as u16)
}

/// 方向键与编辑键的 ANSI 序列。
///
/// PS/2（evdev 键码）与 USB HID 两条输入路径共用同一份序列，避免同一按键
/// 因总线不同产生不同字节。
pub mod ansi {
    /// Esc。
    pub const ESCAPE: &[u8] = b"\x1b";
    /// 上方向键。
    pub const UP: &[u8] = b"\x1b[A";
    /// 下方向键。
    pub const DOWN: &[u8] = b"\x1b[B";
    /// 右方向键。
    pub const RIGHT: &[u8] = b"\x1b[C";
    /// 左方向键。
    pub const LEFT: &[u8] = b"\x1b[D";
    /// Home。
    pub const HOME: &[u8] = b"\x1b[H";
    /// End。
    pub const END: &[u8] = b"\x1b[F";
    /// PageUp。
    pub const PAGE_UP: &[u8] = b"\x1b[5~";
    /// PageDown。
    pub const PAGE_DOWN: &[u8] = b"\x1b[6~";
    /// Insert。
    pub const INSERT: &[u8] = b"\x1b[2~";
    /// Delete。
    pub const DELETE: &[u8] = b"\x1b[3~";
}

/// 应用 Shift 映射。
pub const fn shifted(byte: u8) -> u8 {
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
pub const fn control(byte: u8) -> Option<u8> {
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

/// 按修饰键转换可打印字节；`alt` 以 ESC 前缀表示。
///
/// 返回 `(字节, 长度)`，长度只可能是 1 或 2。
pub const fn modified(byte: u8, shift: bool, ctrl: bool, alt: bool) -> ([u8; 2], usize) {
    let byte = if shift { shifted(byte) } else { byte };
    let byte = if ctrl {
        match control(byte) {
            Some(byte) => byte,
            None => byte,
        }
    } else {
        byte
    };
    if alt {
        ([0x1B, byte], 2)
    } else {
        ([byte, 0], 1)
    }
}

/// 解码后的按键。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// 普通字节，包括 Enter、Backspace 与可打印 ASCII。
    Byte(u8),
    /// 上方向键。
    Up,
    /// 下方向键。
    Down,
    /// 左方向键。
    Left,
    /// 右方向键。
    Right,
    /// Home。
    Home,
    /// End。
    End,
    /// PageUp。
    PageUp,
    /// PageDown。
    PageDown,
    /// Insert。
    Insert,
    /// Delete。
    Delete,
}

/// ANSI 按键序列解码器。
#[derive(Default)]
pub struct Decoder {
    state: u8,
    number: u16,
}

impl Decoder {
    /// 构造解码器。
    pub const fn new() -> Self {
        Self {
            state: 0,
            number: 0,
        }
    }

    /// 送入一个字节；序列完整时返回按键。
    pub fn feed(&mut self, byte: u8) -> Option<Key> {
        match self.state {
            0 => {
                if byte == 0x1B {
                    self.state = 1;
                    None
                } else {
                    Some(Key::Byte(byte))
                }
            }
            1 => {
                if byte == b'[' {
                    self.state = 2;
                    self.number = 0;
                    None
                } else if byte == b'O' {
                    self.state = 3;
                    None
                } else {
                    self.state = 0;
                    Some(Key::Byte(byte))
                }
            }
            2 => {
                if byte.is_ascii_digit() {
                    self.number = self
                        .number
                        .saturating_mul(10)
                        .saturating_add(u16::from(byte - b'0'));
                    None
                } else if byte == b';' {
                    self.number = 0;
                    None
                } else if byte == b'~' {
                    self.state = 0;
                    match self.number {
                        1 | 7 => Some(Key::Home),
                        2 => Some(Key::Insert),
                        3 => Some(Key::Delete),
                        4 | 8 => Some(Key::End),
                        5 => Some(Key::PageUp),
                        6 => Some(Key::PageDown),
                        _ => None,
                    }
                } else {
                    self.state = 0;
                    final_key(byte)
                }
            }
            3 => {
                self.state = 0;
                final_key(byte)
            }
            _ => {
                self.state = 0;
                None
            }
        }
    }
}

fn final_key(byte: u8) -> Option<Key> {
    match byte {
        b'A' => Some(Key::Up),
        b'B' => Some(Key::Down),
        b'C' => Some(Key::Right),
        b'D' => Some(Key::Left),
        b'H' => Some(Key::Home),
        b'F' => Some(Key::End),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #[allow(clippy::wildcard_imports)]
    use super::*;

    fn feed(decoder: &mut Decoder, bytes: &[u8]) -> Option<Key> {
        let mut key = None;
        for &byte in bytes {
            key = decoder.feed(byte);
        }
        key
    }

    #[test]
    fn passes_plain_bytes() {
        let mut decoder = Decoder::new();
        assert_eq!(decoder.feed(b'a'), Some(Key::Byte(b'a')));
        assert_eq!(decoder.feed(b'\n'), Some(Key::Byte(b'\n')));
    }

    #[test]
    fn decodes_csi_sequences() {
        let mut decoder = Decoder::new();
        assert_eq!(feed(&mut decoder, b"\x1b[A"), Some(Key::Up));
        assert_eq!(feed(&mut decoder, b"\x1b[B"), Some(Key::Down));
        assert_eq!(feed(&mut decoder, b"\x1b[C"), Some(Key::Right));
        assert_eq!(feed(&mut decoder, b"\x1b[D"), Some(Key::Left));
        assert_eq!(feed(&mut decoder, b"\x1b[H"), Some(Key::Home));
        assert_eq!(feed(&mut decoder, b"\x1b[F"), Some(Key::End));
        assert_eq!(feed(&mut decoder, b"\x1b[5~"), Some(Key::PageUp));
        assert_eq!(feed(&mut decoder, b"\x1b[6~"), Some(Key::PageDown));
        assert_eq!(feed(&mut decoder, b"\x1b[2~"), Some(Key::Insert));
        assert_eq!(feed(&mut decoder, b"\x1b[3~"), Some(Key::Delete));
    }

    #[test]
    fn ignores_modifier_parameters() {
        let mut decoder = Decoder::new();
        assert_eq!(feed(&mut decoder, b"\x1b[1;5D"), Some(Key::Left));
        assert_eq!(feed(&mut decoder, b"\x1bOA"), Some(Key::Up));
    }

    #[test]
    fn round_trips_wheel_delta() {
        for delta in [-3, -1, 0, 1, 3] {
            assert_eq!(unpack_wheel(pack_wheel(delta)), delta);
        }
    }

    #[test]
    fn shared_sequences_decode_to_their_keys() {
        let cases = [
            (ansi::UP, Key::Up),
            (ansi::DOWN, Key::Down),
            (ansi::RIGHT, Key::Right),
            (ansi::LEFT, Key::Left),
            (ansi::HOME, Key::Home),
            (ansi::END, Key::End),
            (ansi::PAGE_UP, Key::PageUp),
            (ansi::PAGE_DOWN, Key::PageDown),
            (ansi::INSERT, Key::Insert),
            (ansi::DELETE, Key::Delete),
        ];
        for (sequence, expected) in cases {
            let mut decoder = Decoder::new();
            assert_eq!(feed(&mut decoder, sequence), Some(expected), "{sequence:?}");
        }
    }

    #[test]
    fn modifiers_cover_ascii_and_control() {
        assert_eq!(shifted(b'a'), b'A');
        assert_eq!(shifted(b'1'), b'!');
        assert_eq!(shifted(b'/'), b'?');
        assert_eq!(control(b'a'), Some(1));
        assert_eq!(control(b'@'), Some(0));
        assert_eq!(control(b'['), Some(0x1B));
        assert_eq!(control(b'!'), None);

        assert_eq!(modified(b'a', false, false, false), ([b'a', 0], 1));
        assert_eq!(modified(b'a', true, false, false), ([b'A', 0], 1));
        assert_eq!(modified(b'a', false, true, false), ([1, 0], 1));
        assert_eq!(modified(b'a', false, false, true), ([0x1B, b'a'], 2));
        assert_eq!(modified(b'1', true, false, true), ([0x1B, b'!'], 2));
    }
}
