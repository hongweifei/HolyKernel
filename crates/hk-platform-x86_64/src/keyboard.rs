//! PS/2 键盘：i8042 初始化、扫描码解码与输入缓冲。
//!
//! 中断处理只读取数据端口并把 ASCII 字节放入无锁环形缓冲；普通上下文从缓冲取
//! 字节，双方不共享锁，中断可以在任意时刻投递。

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

use crate::port::{inb, outb};

const DATA_PORT: u16 = 0x60;
const STATUS_PORT: u16 = 0x64;
const COMMAND_PORT: u16 = 0x64;

const STATUS_OUTPUT_FULL: u8 = 1 << 0;
const STATUS_INPUT_FULL: u8 = 1 << 1;

const BUFFER_SIZE: usize = 256;

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

static INPUT: InputBuffer = InputBuffer::new();
static SHIFT: AtomicBool = AtomicBool::new(false);
static EXTENDED: AtomicBool = AtomicBool::new(false);

/// 初始化 i8042：启用键盘扫描与 IRQ1，打开扫描码翻译。
pub fn init() {
    // SAFETY: 0x60/0x64 是 i8042 的固定端口；初始化序列只访问这两个端口。
    unsafe {
        wait_input();
        outb(COMMAND_PORT, 0xAD);

        flush_output();

        wait_input();
        outb(COMMAND_PORT, 0x20);
        let mut config = read_data();
        config |= 0x01 | 0x40;
        config &= !0x10;

        wait_input();
        outb(COMMAND_PORT, 0x60);
        wait_input();
        outb(DATA_PORT, config);

        wait_input();
        outb(COMMAND_PORT, 0xAE);

        flush_output();
    }
}

/// 从输入缓冲取出一个字节。
pub fn read() -> Option<u8> {
    INPUT.pop()
}

/// 处理键盘中断：读取扫描码并解码。
pub fn handle_interrupt() {
    // SAFETY: 状态端口与数据端口是 i8042 的固定端口；只读取一个到达的字节。
    unsafe {
        if inb(STATUS_PORT) & STATUS_OUTPUT_FULL == 0 {
            return;
        }
        let scancode = inb(DATA_PORT);
        decode(scancode);
    }
}

fn decode(scancode: u8) {
    if scancode == SCANCODE_EXTENDED {
        EXTENDED.store(true, Ordering::Relaxed);
        return;
    }
    if EXTENDED.swap(false, Ordering::Relaxed) {
        return;
    }

    if scancode & SCANCODE_RELEASE != 0 {
        match scancode {
            SCANCODE_SHIFT_RELEASE_LEFT | SCANCODE_SHIFT_RELEASE_RIGHT => {
                SHIFT.store(false, Ordering::Relaxed);
            }
            _ => {}
        }
        return;
    }

    match scancode {
        SCANCODE_SHIFT_LEFT | SCANCODE_SHIFT_RIGHT => {
            SHIFT.store(true, Ordering::Relaxed);
        }
        KEY_ENTER => INPUT.push(b'\n'),
        KEY_BACKSPACE => INPUT.push(0x08),
        KEY_SPACE => INPUT.push(b' '),
        _ => {
            if let Some(byte) = translate(scancode, SHIFT.load(Ordering::Relaxed)) {
                INPUT.push(byte);
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

fn wait_input() {
    for _ in 0..100_000 {
        // SAFETY: 状态端口是 i8042 的固定端口。
        if unsafe { inb(STATUS_PORT) } & STATUS_INPUT_FULL == 0 {
            return;
        }
        core::hint::spin_loop();
    }
}

fn read_data() -> u8 {
    for _ in 0..100_000 {
        // SAFETY: 状态端口与数据端口是 i8042 的固定端口。
        unsafe {
            if inb(STATUS_PORT) & STATUS_OUTPUT_FULL != 0 {
                return inb(DATA_PORT);
            }
        }
        core::hint::spin_loop();
    }
    0
}

fn flush_output() {
    for _ in 0..64 {
        // SAFETY: 状态端口与数据端口是 i8042 的固定端口。
        unsafe {
            if inb(STATUS_PORT) & STATUS_OUTPUT_FULL == 0 {
                return;
            }
            let _ = inb(DATA_PORT);
        }
    }
}
