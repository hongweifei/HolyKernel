//! PS/2 键盘：i8042 初始化、扫描码解码与输入缓冲。
//!
//! 平台通过 [`PortIo`] 提供端口或 MMIO 访问；本 crate 不依赖架构。中断处理只
//! 读取数据端口并把 ASCII 字节放入无锁环形缓冲。

#![no_std]

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

use hk_hal::Io;

const BUFFER_SIZE: usize = 256;

const STATUS_OUTPUT_FULL: u8 = 1 << 0;
const STATUS_INPUT_FULL: u8 = 1 << 1;

const SCANCODE_EXTENDED: u8 = 0xE0;
const SCANCODE_SHIFT_LEFT: u8 = 0x2A;
const SCANCODE_SHIFT_RIGHT: u8 = 0x36;
const SCANCODE_SHIFT_RELEASE_LEFT: u8 = 0xAA;
const SCANCODE_SHIFT_RELEASE_RIGHT: u8 = 0xB6;
const SCANCODE_RELEASE: u8 = 0x80;

const KEY_1: u8 = 0x02;
const KEY_0: u8 = 0x0B;
const KEY_BACKSPACE: u8 = 0x0E;
const KEY_ENTER: u8 = 0x1C;
const KEY_SPACE: u8 = 0x39;

/// 键盘控制器端口。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ports {
    /// 数据端口。
    pub data: u16,
    /// 状态端口。
    pub status: u16,
    /// 命令端口。
    pub command: u16,
}

impl Ports {
    /// 标准 i8042 端口。
    pub const I8042: Self = Self {
        data: 0x60,
        status: 0x64,
        command: 0x64,
    };
}

struct InputBuffer {
    data: [AtomicU8; BUFFER_SIZE],
    head: AtomicUsize,
    tail: AtomicUsize,
}

impl InputBuffer {
    const fn new() -> Self {
        Self {
            data: [const { AtomicU8::new(0) }; BUFFER_SIZE],
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    fn push(&self, byte: u8) {
        let head = self.head.load(Ordering::Relaxed);
        let next = (head + 1) % BUFFER_SIZE;
        if next == self.tail.load(Ordering::Acquire) {
            return;
        }
        self.data[head].store(byte, Ordering::Relaxed);
        self.head.store(next, Ordering::Release);
    }

    fn pop(&self) -> Option<u8> {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail == self.head.load(Ordering::Acquire) {
            return None;
        }
        let byte = self.data[tail].load(Ordering::Relaxed);
        self.tail.store((tail + 1) % BUFFER_SIZE, Ordering::Release);
        Some(byte)
    }
}

/// PS/2 键盘。
pub struct Ps2Keyboard<P> {
    port: P,
    ports: Ports,
    input: InputBuffer,
    shift: AtomicBool,
    extended: AtomicBool,
}

impl<P: Io> Ps2Keyboard<P> {
    /// 构造键盘驱动。
    pub const fn new(port: P, ports: Ports) -> Self {
        Self {
            port,
            ports,
            input: InputBuffer::new(),
            shift: AtomicBool::new(false),
            extended: AtomicBool::new(false),
        }
    }

    /// 初始化 i8042：启用键盘扫描与 IRQ1，打开扫描码翻译。
    pub fn init(&self) {
        // SAFETY: 端口由平台提供，初始化序列只访问键盘控制器寄存器。
        unsafe {
            self.wait_input();
            self.port.write8(u64::from(self.ports.command), 0xAD);

            self.flush_output();

            self.wait_input();
            self.port.write8(u64::from(self.ports.command), 0x20);
            let mut config = self.read_data();
            config |= 0x01 | 0x40;
            config &= !0x10;

            self.wait_input();
            self.port.write8(u64::from(self.ports.command), 0x60);
            self.wait_input();
            self.port.write8(u64::from(self.ports.data), config);

            self.wait_input();
            self.port.write8(u64::from(self.ports.command), 0xAE);

            self.flush_output();
        }
    }

    /// 从输入缓冲取出一个字节。
    pub fn read(&self) -> Option<u8> {
        self.input.pop()
    }

    /// 处理键盘中断：读取扫描码并解码。
    pub fn handle_interrupt(&self) {
        // SAFETY: 状态端口与数据端口由平台提供；只读取一个到达的字节。
        unsafe {
            if self.port.read8(u64::from(self.ports.status)) & STATUS_OUTPUT_FULL == 0 {
                return;
            }
            let scancode = self.port.read8(u64::from(self.ports.data));
            self.decode(scancode);
        }
    }

    /// 解码一个扫描码。
    pub fn decode(&self, scancode: u8) {
        if scancode == SCANCODE_EXTENDED {
            self.extended.store(true, Ordering::Relaxed);
            return;
        }
        if self.extended.swap(false, Ordering::Relaxed) {
            return;
        }

        if scancode & SCANCODE_RELEASE != 0 {
            match scancode {
                SCANCODE_SHIFT_RELEASE_LEFT | SCANCODE_SHIFT_RELEASE_RIGHT => {
                    self.shift.store(false, Ordering::Relaxed);
                }
                _ => {}
            }
            return;
        }

        match scancode {
            SCANCODE_SHIFT_LEFT | SCANCODE_SHIFT_RIGHT => {
                self.shift.store(true, Ordering::Relaxed);
            }
            KEY_ENTER => self.input.push(b'\n'),
            KEY_BACKSPACE => self.input.push(0x08),
            KEY_SPACE => self.input.push(b' '),
            _ => {
                if let Some(byte) = translate(scancode, self.shift.load(Ordering::Relaxed)) {
                    self.input.push(byte);
                }
            }
        }
    }

    fn wait_input(&self) {
        for _ in 0..100_000 {
            // SAFETY: 端口由平台提供，只读取状态寄存器。
            if unsafe { self.port.read8(u64::from(self.ports.status)) } & STATUS_INPUT_FULL == 0 {
                return;
            }
            core::hint::spin_loop();
        }
    }

    fn read_data(&self) -> u8 {
        for _ in 0..100_000 {
            // SAFETY: 端口由平台提供，只读取状态与数据寄存器。
            unsafe {
                if self.port.read8(u64::from(self.ports.status)) & STATUS_OUTPUT_FULL != 0 {
                    return self.port.read8(u64::from(self.ports.data));
                }
            }
            core::hint::spin_loop();
        }
        0
    }

    fn flush_output(&self) {
        for _ in 0..64 {
            // SAFETY: 端口由平台提供，只读取状态与数据寄存器。
            unsafe {
                if self.port.read8(u64::from(self.ports.status)) & STATUS_OUTPUT_FULL == 0 {
                    return;
                }
                let _ = self.port.read8(u64::from(self.ports.data));
            }
        }
    }
}

fn translate(scancode: u8, shift: bool) -> Option<u8> {
    let base = match scancode {
        KEY_1..=KEY_0 => b'1' + (scancode - KEY_1),
        0x0C => b'-',
        0x0D => b'=',
        0x10..=0x19 => b"qwertyuiop"[usize::from(scancode - 0x10)],
        0x1A => b'[',
        0x1B => b']',
        0x1E..=0x26 => b"asdfghjkl"[usize::from(scancode - 0x1E)],
        0x27 => b';',
        0x28 => b'\'',
        0x29 => b'`',
        0x2B => b'\\',
        0x2C..=0x32 => b"zxcvbnm"[usize::from(scancode - 0x2C)],
        0x33 => b',',
        0x34 => b'.',
        0x35 => b'/',
        _ => return None,
    };
    Some(if shift { shifted(base) } else { base })
}

fn shifted(base: u8) -> u8 {
    match base {
        b'a'..=b'z' => base - 32,
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

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::AtomicU8;

    struct MockPort {
        status: AtomicU8,
        data: AtomicU8,
    }

    impl Io for MockPort {
        unsafe fn read8(&self, address: u64) -> u8 {
            if address == u64::from(Ports::I8042.status) {
                self.status.load(Ordering::Relaxed)
            } else {
                self.data.load(Ordering::Relaxed)
            }
        }

        unsafe fn write8(&self, address: u64, value: u8) {
            if address == u64::from(Ports::I8042.data) {
                self.data.store(value, Ordering::Relaxed);
            }
        }

        unsafe fn read32(&self, _address: u64) -> u32 {
            0
        }

        unsafe fn write32(&self, _address: u64, _value: u32) {}
    }

    fn keyboard() -> Ps2Keyboard<MockPort> {
        Ps2Keyboard::new(
            MockPort {
                status: AtomicU8::new(0),
                data: AtomicU8::new(0),
            },
            Ports::I8042,
        )
    }

    #[test]
    fn decodes_letters_and_shift() {
        let keyboard = keyboard();
        keyboard.decode(0x1E);
        assert_eq!(keyboard.read(), Some(b'a'));

        keyboard.decode(0x2A);
        keyboard.decode(0x1E);
        assert_eq!(keyboard.read(), Some(b'A'));

        keyboard.decode(0xAA);
        keyboard.decode(0x1E);
        assert_eq!(keyboard.read(), Some(b'a'));
    }

    #[test]
    fn decodes_navigation_keys() {
        let keyboard = keyboard();
        keyboard.decode(KEY_ENTER);
        keyboard.decode(KEY_BACKSPACE);
        keyboard.decode(KEY_SPACE);
        assert_eq!(keyboard.read(), Some(b'\n'));
        assert_eq!(keyboard.read(), Some(0x08));
        assert_eq!(keyboard.read(), Some(b' '));
    }
}
