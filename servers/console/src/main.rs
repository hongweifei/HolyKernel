//! 控制台服务：读取内核输入、做行编辑与回显，并向客户端提供写文本/读一行。
//!
//! 有 Display 服务时，Console 创建自己的窗口，把文本渲染到共享表面并提交
//! 合成；没有 Display 时回退到内核控制台输出。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;
#[cfg(target_os = "none")]
use hk_server_api::console::{
    CONSOLE_READ_LINE, CONSOLE_READ_REQUEST, CONSOLE_SHUTDOWN, CONSOLE_WRITE, CONSOLE_WRITE_ACK,
};
#[cfg(target_os = "none")]
use hk_server_api::display::{
    DISPLAY_COMMIT, DISPLAY_CREATE_WINDOW, DISPLAY_FRAME_DONE, DISPLAY_KEY, DISPLAY_SERVICE,
    DISPLAY_WINDOW_CREATED, pack_size, unpack_key,
};
#[cfg(target_os = "none")]
use hk_server_api::{
    console_read, exit, frame_alloc, recv_data_try, send, send_data, send_handle, service_endpoint,
    write, write_u64, yield_now,
};
#[cfg(target_os = "none")]
use lovyangfx_fonts::Font;
#[cfg(target_os = "none")]
use lovyangfx_fonts::fonts::efont_cn_16;

/// 终端列数。
#[cfg(target_os = "none")]
const COLUMNS: usize = 80;
/// 终端行数。
#[cfg(target_os = "none")]
const ROWS: usize = 24;
/// 单元格宽度。
#[cfg(target_os = "none")]
const CELL_WIDTH: usize = 8;
/// 单元格高度。
#[cfg(target_os = "none")]
const CELL_HEIGHT: usize = 18;
/// 表面宽度（像素）。
#[cfg(target_os = "none")]
const WIDTH: usize = COLUMNS * CELL_WIDTH;
/// 表面高度（像素）。
#[cfg(target_os = "none")]
const HEIGHT: usize = ROWS * CELL_HEIGHT;
/// 表面映射地址。
#[cfg(target_os = "none")]
const SURFACE: u64 = 0x5000_0000;

/// 终端背景色。
#[cfg(target_os = "none")]
const BACKGROUND: u32 = 0x0010_1010;
/// 终端前景色。
#[cfg(target_os = "none")]
const FOREGROUND: u32 = 0x00D0_D0D0;

/// 图形终端状态。
#[cfg(target_os = "none")]
struct Terminal {
    display: u64,
    window: u64,
    pixels: *mut u32,
    grid: [[u8; COLUMNS]; ROWS],
    column: usize,
    row: usize,
    font: Font<'static>,
}

#[cfg(target_os = "none")]
impl Terminal {
    /// 写入一段文本并提交合成。
    fn emit(&mut self, text: &[u8]) {
        for byte in text {
            self.put(*byte);
        }
        self.refresh();
    }

    fn put(&mut self, byte: u8) {
        match byte {
            b'\n' => {
                self.column = 0;
                self.row += 1;
            }
            b'\r' => self.column = 0,
            b'\t' => {
                for _ in 0..4 {
                    self.put(b' ');
                }
            }
            0x08 => {
                if self.column > 0 {
                    self.column -= 1;
                    self.grid[self.row][self.column] = b' ';
                }
            }
            0x20..=0x7E => {
                if self.column >= COLUMNS {
                    self.column = 0;
                    self.row += 1;
                }
                self.grid[self.row][self.column] = byte;
                self.column += 1;
            }
            _ => {}
        }
        while self.row >= ROWS {
            self.grid[..].copy_within(1.., 0);
            self.grid[ROWS - 1] = [b' '; COLUMNS];
            self.row = ROWS - 1;
        }
    }

    /// 重绘整个网格并提交。
    fn refresh(&mut self) {
        for row in 0..ROWS {
            for column in 0..COLUMNS {
                self.draw_cell(row, column);
            }
        }
        let _ = send(self.display, DISPLAY_COMMIT, self.window);
    }

    fn draw_cell(&self, row: usize, column: usize) {
        let origin_x = column * CELL_WIDTH;
        let origin_y = row * CELL_HEIGHT;
        for y in 0..CELL_HEIGHT {
            for x in 0..CELL_WIDTH {
                self.put_pixel(origin_x + x, origin_y + y, BACKGROUND);
            }
        }
        let byte = self.grid[row][column];
        if byte == b' ' {
            return;
        }
        let Some(glyph) = self.font.glyph(byte as char) else {
            return;
        };
        glyph.for_each_pixel(|x, y| {
            let pixel_x = origin_x as i32 + x;
            let pixel_y = origin_y as i32 + y;
            if pixel_x >= 0 && pixel_y >= 0 {
                self.put_pixel(pixel_x as usize, pixel_y as usize, FOREGROUND);
            }
        });
    }

    fn put_pixel(&self, x: usize, y: usize, color: u32) {
        if x >= WIDTH || y >= HEIGHT {
            return;
        }
        // SAFETY: 共享表面已按 WIDTH×HEIGHT 映射，坐标已裁剪。
        unsafe {
            self.pixels.add(y * WIDTH + x).write_volatile(color);
        }
    }
}

/// 尝试连接 Display 并创建终端窗口。
#[cfg(target_os = "none")]
fn attach_terminal(endpoint: u64) -> Option<Terminal> {
    let display = service_endpoint(DISPLAY_SERVICE);
    if display == 0 {
        return None;
    }
    let handle = frame_alloc(WIDTH * HEIGHT * 4, SURFACE);
    if handle == 0 || handle == u64::MAX {
        return None;
    }
    if send_handle(
        display,
        DISPLAY_CREATE_WINDOW,
        pack_size(WIDTH as u32, HEIGHT as u32),
        handle,
    ) == u64::MAX
    {
        return None;
    }
    // 只等待窗口创建回复；显示服务启动期间可能已有其他消息到达。
    let mut payload = [0_u8; 8];
    let mut window = 0_u64;
    for _ in 0..4096 {
        let (label, word, _sender, _size) = recv_data_try(endpoint, &mut payload);
        if label == DISPLAY_WINDOW_CREATED {
            window = word;
            break;
        }
        let _ = yield_now();
    }
    if window == 0 {
        return None;
    }
    Some(Terminal {
        display,
        window,
        pixels: SURFACE as *mut u32,
        grid: [[b' '; COLUMNS]; ROWS],
        column: 0,
        row: 0,
        font: efont_cn_16(),
    })
}

/// 输出到图形终端；没有终端时退回内核控制台。
#[cfg(target_os = "none")]
fn emit(terminal: &mut Option<Terminal>, text: &[u8]) {
    match terminal {
        Some(terminal) => terminal.emit(text),
        None => write(text),
    }
}

/// 处理一个输入字节：行编辑、回显与按行回复。
#[cfg(target_os = "none")]
fn handle_input(
    terminal: &mut Option<Terminal>,
    line: &mut [u8; 128],
    length: &mut usize,
    pending: &mut u64,
    byte: u8,
) {
    match byte {
        b'\n' | b'\r' => {
            emit(terminal, b"\n");
            if *pending != 0 {
                let _ = send_data(*pending, CONSOLE_READ_LINE, &line[..*length]);
                *pending = 0;
            }
            *length = 0;
        }
        0x08 | 0x7F => {
            if *length > 0 {
                *length -= 1;
                emit(terminal, b"\x08 \x08");
            }
        }
        byte if (0x20..0x7F).contains(&byte) => {
            if *length < line.len() {
                line[*length] = byte;
                *length += 1;
                emit(terminal, &[byte]);
            }
        }
        _ => {}
    }
}

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let mut terminal = attach_terminal(endpoint);
    if let Some(terminal) = &mut terminal {
        write(b"hk-server-console: window ");
        write_u64(terminal.window);
        write(b"\n");
        terminal.refresh();
    }

    let mut buffer = [0_u8; 256];
    let mut line = [0_u8; 128];
    let mut length = 0_usize;
    let mut pending = 0_u64;

    loop {
        loop {
            let (label, word, sender, size) = recv_data_try(endpoint, &mut buffer);
            if label == 0 {
                break;
            }
            match label {
                CONSOLE_WRITE => {
                    let text = &buffer[..size.min(buffer.len())];
                    emit(&mut terminal, text);
                    if terminal.is_some() {
                        let _ = send(sender, CONSOLE_WRITE_ACK, 0);
                    }
                }
                CONSOLE_READ_REQUEST => pending = sender,
                CONSOLE_SHUTDOWN => exit(0),
                DISPLAY_KEY => {
                    let (_, byte) = unpack_key(word);
                    handle_input(&mut terminal, &mut line, &mut length, &mut pending, byte);
                }
                DISPLAY_FRAME_DONE => {}
                _ => {}
            }
        }

        // 有 Display 时，键盘事件经焦点窗口路由到本服务；无 Display 时直接读内核输入。
        if terminal.is_none() {
            while let Some(byte) = console_read() {
                handle_input(&mut terminal, &mut line, &mut length, &mut pending, byte);
            }
        }

        let _ = yield_now();
    }
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(not(target_os = "none"))]
fn main() {
    eprintln!("hk-server-console 只在 x86_64-unknown-none 目标上构建");
}
