//! 输入设备事件协议。
//!
//! 用户态输入驱动把按键字节序列、指针位置与滚轮增量发给显示服务，由显示
//! 服务按焦点路由到窗口客户端；特殊键使用 ANSI 序列，消费端用 [`Decoder`]
//! 还原。内核控制台输入在输入驱动模式下关闭。

/// 按键事件；`word` 为字节值。
pub const INPUT_KEY: u64 = 0xB00;
/// 绝对指针位置；`word` 为打包的 16 位 x/y（0..=0x7FFF）。
pub const INPUT_POINTER: u64 = 0xB01;
/// 指针按钮；`word` 非 0 为按下。
pub const INPUT_BUTTON: u64 = 0xB02;
/// 鼠标滚轮；`word` 为有符号增量，正值向上。
pub const INPUT_WHEEL: u64 = 0xB03;

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
}
