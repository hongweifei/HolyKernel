//! 图形终端模型：字符网格、UTF-8 解码与共享表面渲染。

use hk_server_api::display::{
    DISPLAY_COMMIT, DISPLAY_CREATE_WINDOW, DISPLAY_SERVICE, DISPLAY_WINDOW_CREATED, pack_size,
};
use hk_server_api::{
    frame_alloc, recv_data_try, send, send_handle, service_endpoint, write, yield_now,
};
use lovyangfx_fonts::Font;
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
/// 光标颜色。
#[cfg(target_os = "none")]
const CURSOR: u32 = 0x0060_A0E0;

/// 增量 UTF-8 解码器。
#[cfg(target_os = "none")]
struct Utf8 {
    pending: [u8; 4],
    length: usize,
}

#[cfg(target_os = "none")]
impl Utf8 {
    const fn new() -> Self {
        Self {
            pending: [0; 4],
            length: 0,
        }
    }

    /// 推入一个字节；序列完整时返回字符。
    fn push(&mut self, byte: u8) -> Option<char> {
        if self.length == 0 {
            if byte < 0x80 {
                return Some(byte as char);
            }
            if byte & 0xC0 == 0x80 {
                return Some('?');
            }
            self.pending[0] = byte;
            self.length = 1;
            return None;
        }
        self.pending[self.length] = byte;
        self.length += 1;
        let needed = match self.pending[0] {
            0xC0..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF7 => 4,
            _ => {
                self.length = 0;
                return Some('?');
            }
        };
        if self.length < needed {
            return None;
        }
        let value = core::str::from_utf8(&self.pending[..needed])
            .ok()
            .and_then(|text| text.chars().next());
        self.length = 0;
        Some(value.unwrap_or('?'))
    }
}

/// 图形终端状态。
#[cfg(target_os = "none")]
pub(crate) struct Terminal {
    display: u64,
    window: u64,
    pixels: *mut u32,
    grid: [[char; COLUMNS]; ROWS],
    column: usize,
    row: usize,
    font: Font<'static>,
    decoder: Utf8,
    utf8_seen: bool,
    cjk_seen: bool,
}

#[cfg(target_os = "none")]
impl Terminal {
    /// 写入一段文本并提交合成。
    pub(crate) fn emit(&mut self, text: &[u8]) {
        for byte in text {
            self.put_byte(*byte);
        }
        self.refresh();
    }

    fn put_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => {
                self.column = 0;
                self.row += 1;
            }
            b'\r' => self.column = 0,
            b'\t' => {
                for _ in 0..4 {
                    self.put_char(' ');
                }
            }
            0x08 | 0x7F => self.backspace(),
            byte if byte >= 0x20 => {
                if let Some(character) = self.decoder.push(byte) {
                    self.put_char(character);
                }
            }
            _ => {}
        }
        self.wrap();
    }

    fn put_char(&mut self, character: char) {
        let width = self.advance(character);
        if u32::from(character) > 0x7F && !self.utf8_seen {
            self.utf8_seen = true;
            write(b"hk-server-console: utf8 ok\n");
        }
        if width > 1 && !self.cjk_seen {
            self.cjk_seen = true;
            write(b"hk-server-console: cjk ok\n");
        }
        if self.column + width > COLUMNS {
            self.column = 0;
            self.row += 1;
            self.wrap();
        }
        self.grid[self.row][self.column] = character;
        if width > 1 {
            self.grid[self.row][self.column + 1] = '\0';
        }
        self.column += width;
    }

    fn advance(&self, character: char) -> usize {
        self.font.glyph(character).map_or(1, |glyph| {
            if glyph.metric().xadvance.max(0) as usize > CELL_WIDTH {
                2
            } else {
                1
            }
        })
    }

    fn backspace(&mut self) {
        if self.column == 0 {
            return;
        }
        self.column -= 1;
        if self.grid[self.row][self.column] == '\0' && self.column > 0 {
            self.grid[self.row][self.column] = ' ';
            self.column -= 1;
        }
        self.grid[self.row][self.column] = ' ';
    }

    fn wrap(&mut self) {
        while self.row >= ROWS {
            self.grid[..].copy_within(1.., 0);
            self.grid[ROWS - 1] = [' '; COLUMNS];
            self.row = ROWS - 1;
        }
    }

    /// 重绘整个网格与光标，并提交合成。
    fn refresh(&mut self) {
        for row in 0..ROWS {
            for column in 0..COLUMNS {
                for y in 0..CELL_HEIGHT {
                    for x in 0..CELL_WIDTH {
                        self.put_pixel(column * CELL_WIDTH + x, row * CELL_HEIGHT + y, BACKGROUND);
                    }
                }
            }
        }
        for row in 0..ROWS {
            for column in 0..COLUMNS {
                let character = self.grid[row][column];
                if character == ' ' || character == '\0' {
                    continue;
                }
                let Some(glyph) = self.font.glyph(character) else {
                    continue;
                };
                let origin_x = (column * CELL_WIDTH) as i32;
                let origin_y = (row * CELL_HEIGHT) as i32;
                glyph.for_each_pixel(|x, y| {
                    let pixel_x = origin_x + x;
                    let pixel_y = origin_y + y;
                    if pixel_x >= 0 && pixel_y >= 0 {
                        self.put_pixel(pixel_x as usize, pixel_y as usize, FOREGROUND);
                    }
                });
            }
        }
        let cursor_y = self.row * CELL_HEIGHT + CELL_HEIGHT - 2;
        let cursor_x = self.column * CELL_WIDTH;
        for y in 0..2 {
            for x in 0..CELL_WIDTH {
                self.put_pixel(cursor_x + x, cursor_y + y, CURSOR);
            }
        }
        let _ = send(self.display, DISPLAY_COMMIT, self.window);
    }

    /// 返回窗口编号。
    pub(crate) fn window(&self) -> u64 {
        self.window
    }

    /// 返回 Display 端点。
    pub(crate) fn display(&self) -> u64 {
        self.display
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
pub(crate) fn attach_terminal(endpoint: u64) -> Option<Terminal> {
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
        grid: [[' '; COLUMNS]; ROWS],
        column: 0,
        row: 0,
        font: efont_cn_16(),
        decoder: Utf8::new(),
        utf8_seen: false,
        cjk_seen: false,
    })
}
