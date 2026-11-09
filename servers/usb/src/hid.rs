//! USB HID boot protocol 报表解析。
//!
//! 只处理 HID boot keyboard/mouse；报表格式由 USB HID 规范固定，避免依赖
//! 各设备私有的 report descriptor。非 boot 设备由服务在枚举时跳过。

use alloc::vec::Vec;

/// 需要转发给显示服务的输入事件。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// 一个待发送的字节。
    Key(u8),
    /// 绝对指针位置。
    Pointer(u16, u16),
    /// 左键状态。
    Button(bool),
    /// 滚轮增量。
    Wheel(i32),
}

/// HID boot keyboard 状态。
#[derive(Default)]
pub struct KeyboardState {
    previous: [u8; 6],
}

impl KeyboardState {
    /// 解析 8 字节 boot keyboard 报表。
    pub fn report(&mut self, report: &[u8], out: &mut Vec<Event>) {
        if report.len() < 8 {
            return;
        }
        let modifiers = report[0];
        let shift = modifiers & 0x22 != 0;
        let ctrl = modifiers & 0x11 != 0;
        let alt = modifiers & 0x44 != 0;

        for &usage in &report[2..8] {
            if usage == 0 || self.previous.contains(&usage) {
                continue;
            }
            emit_usage(usage, shift, ctrl, alt, out);
        }
        self.previous.copy_from_slice(&report[2..8]);
    }
}

/// HID boot mouse 状态。
pub struct MouseState {
    x: i32,
    y: i32,
    buttons: u8,
}

impl Default for MouseState {
    fn default() -> Self {
        Self {
            x: i32::from(u16::MAX / 2),
            y: i32::from(u16::MAX / 2),
            buttons: 0,
        }
    }
}

impl MouseState {
    /// 解析 3/4 字节 boot mouse 报表。
    pub fn report(&mut self, report: &[u8], out: &mut Vec<Event>) {
        if report.len() < 3 {
            return;
        }
        let buttons = report[0];
        let dx = i32::from(report[1] as i8);
        let dy = i32::from(report[2] as i8);
        if dx != 0 || dy != 0 {
            self.x = (self.x + dx * 128).clamp(0, i32::from(u16::MAX));
            self.y = (self.y - dy * 128).clamp(0, i32::from(u16::MAX));
            out.push(Event::Pointer(self.x as u16, self.y as u16));
        }
        let left = buttons & 1 != 0;
        let old_left = self.buttons & 1 != 0;
        if left != old_left {
            out.push(Event::Button(left));
        }
        self.buttons = buttons;
        if report.len() >= 4 && report[3] != 0 {
            out.push(Event::Wheel(i32::from(report[3] as i8)));
        }
    }
}

fn emit_usage(usage: u8, shift: bool, ctrl: bool, alt: bool, out: &mut Vec<Event>) {
    if let Some(sequence) = special(usage) {
        for &byte in sequence {
            out.push(Event::Key(byte));
        }
        return;
    }
    let Some(plain) = printable(usage) else {
        return;
    };
    let byte = if shift { shifted(plain) } else { plain };
    let byte = if ctrl {
        control(byte).unwrap_or(byte)
    } else {
        byte
    };
    if alt {
        out.push(Event::Key(0x1B));
    }
    out.push(Event::Key(byte));
}

fn special(usage: u8) -> Option<&'static [u8]> {
    Some(match usage {
        0x28 => &b"\n"[..],
        0x29 => &b"\x1b"[..],
        0x2A => &b"\x08"[..],
        0x2B => &b"\t"[..],
        0x49 => &b"\x1b[2~"[..],
        0x4A => &b"\x1b[H"[..],
        0x4B => &b"\x1b[5~"[..],
        0x4C => &b"\x1b[3~"[..],
        0x4D => &b"\x1b[F"[..],
        0x4E => &b"\x1b[6~"[..],
        0x4F => &b"\x1b[C"[..],
        0x50 => &b"\x1b[D"[..],
        0x51 => &b"\x1b[B"[..],
        0x52 => &b"\x1b[A"[..],
        _ => return None,
    })
}

fn printable(usage: u8) -> Option<u8> {
    Some(match usage {
        0x04..=0x1D => b'a' + (usage - 0x04),
        0x1E..=0x26 => b'1' + (usage - 0x1E),
        0x27 => b'0',
        0x2C => b' ',
        0x2D => b'-',
        0x2E => b'=',
        0x2F => b'[',
        0x30 => b']',
        0x31 => b'\\',
        0x32 => b';',
        0x33 => b'\'',
        0x34 => b'`',
        0x35 => b',',
        0x36 => b'.',
        0x37 => b'/',
        _ => return None,
    })
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_emits_pressed_keys_with_shift() {
        let mut state = KeyboardState::default();
        let mut events = Vec::new();
        state.report(&[0x02, 0, 0x04, 0x05, 0, 0, 0, 0], &mut events);
        assert_eq!(events, [Event::Key(b'A'), Event::Key(b'B')]);

        events.clear();
        state.report(&[0x02, 0, 0x04, 0x05, 0, 0, 0, 0], &mut events);
        assert!(events.is_empty());
    }

    #[test]
    fn keyboard_maps_arrows_and_alt() {
        let mut state = KeyboardState::default();
        let mut events = Vec::new();
        state.report(&[0, 0, 0x52, 0, 0, 0, 0, 0], &mut events);
        assert_eq!(
            events,
            [Event::Key(0x1B), Event::Key(b'['), Event::Key(b'A')]
        );

        events.clear();
        state.report(&[0, 0, 0, 0, 0, 0, 0, 0], &mut events);
        state.report(&[0x04, 0, 0x04, 0, 0, 0, 0, 0], &mut events);
        assert_eq!(events, [Event::Key(0x1B), Event::Key(b'a')]);
    }

    #[test]
    fn mouse_accumulates_motion_and_buttons() {
        let mut state = MouseState::default();
        let mut events = Vec::new();
        state.report(&[1, 2, 0xFF], &mut events);
        let center = i32::from(u16::MAX / 2);
        assert_eq!(
            events,
            [
                Event::Pointer((center + 256) as u16, (center + 128) as u16),
                Event::Button(true),
            ]
        );

        events.clear();
        state.report(&[1, 0, 0, 1], &mut events);
        assert_eq!(events, [Event::Wheel(1)]);
    }
}
