//! 图形终端：带样式的字符网格、UTF-8 解码、ANSI 命令与窗口生命周期。

mod ansi;
mod render;
mod style;

use hk_canvas::{Surface, default_monospace};
use hk_font::Font;
use hk_server_api::display::{
    DISPLAY_SERVICE, SURFACE_COMMIT, SURFACE_CREATE, SURFACE_CREATED, WINDOW_CREATE,
    WINDOW_CREATED, WINDOW_RESIZE, pack_size, pack_window_size,
};
use hk_server_api::{
    CAPABILITY_SURFACE, close_handle, map_new, recv_data_try, region_unmap, send, send_handle,
    service_endpoint, write, yield_now,
};

use ansi::{Ansi, Command, Event};
use style::{Cell, Style};

/// 终端初始列数。
pub(crate) const COLUMNS: usize = 80;
/// 终端初始行数。
pub(crate) const ROWS: usize = 24;
/// 网格最大列数。
pub(crate) const MAX_COLUMNS: usize = 240;
/// 网格最大行数。
pub(crate) const MAX_ROWS: usize = 60;
/// 单元格宽度。
pub(crate) const CELL_WIDTH: usize = 8;
/// 单元格高度。
pub(crate) const CELL_HEIGHT: usize = 18;
/// 初始表面宽度（像素）。
pub(crate) const WIDTH: usize = COLUMNS * CELL_WIDTH;
/// 初始表面高度（像素）。
pub(crate) const HEIGHT: usize = ROWS * CELL_HEIGHT;

/// 字符网格。
pub(crate) type Grid = [[Cell; MAX_COLUMNS]; MAX_ROWS];

/// 全局字符网格；Console 单线程持有唯一访问权。
static mut GRID: Grid = [[Cell::BLANK; MAX_COLUMNS]; MAX_ROWS];

/// 返回全局网格；一次方法调用内完成访问，不跨调用持有。
fn grid() -> &'static mut Grid {
    // SAFETY: Console 服务单线程运行，网格访问都在终端方法内串行完成。
    unsafe { &mut *core::ptr::addr_of_mut!(GRID) }
}

/// 清空全局网格。
fn reset_grid() {
    grid().fill([Cell::BLANK; MAX_COLUMNS]);
}

/// 增量 UTF-8 解码器。
struct Utf8 {
    pending: [u8; 4],
    length: usize,
}

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
pub(crate) struct Terminal {
    display: u64,
    surface: u64,
    window: u64,
    surface_virt: u64,
    surface_region: u64,
    pixels: *mut u32,
    width: usize,
    height: usize,
    columns: usize,
    rows: usize,
    column: usize,
    row: usize,
    font: &'static Font,
    decoder: Utf8,
    style: Style,
    ansi: Ansi,
    sgr_seen: bool,
    utf8_seen: bool,
    cjk_seen: bool,
}

impl Terminal {
    /// 写入一段文本并提交合成。
    pub(crate) fn emit(&mut self, text: &[u8]) {
        for byte in text {
            self.put_byte(*byte);
        }
        self.refresh();
    }

    fn put_byte(&mut self, byte: u8) {
        match self.ansi.feed(byte) {
            Event::Byte(byte) => self.put_ground(byte),
            Event::Command(command) => self.apply(command),
            Event::Ignore => {}
        }
    }

    fn put_ground(&mut self, byte: u8) {
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

    fn apply(&mut self, command: Command) {
        match command {
            Command::Sgr { params, count } => {
                if !self.sgr_seen {
                    self.sgr_seen = true;
                    write(b"hk-server-console: sgr ok\n");
                }
                self.style.apply(&params[..count]);
            }
            Command::CursorPosition { row, column } => {
                self.row = usize::from(row.saturating_sub(1)).min(self.rows.saturating_sub(1));
                self.column =
                    usize::from(column.saturating_sub(1)).min(self.columns.saturating_sub(1));
            }
            Command::CursorUp(lines) => {
                self.row = self.row.saturating_sub(usize::from(lines));
            }
            Command::CursorDown(lines) => {
                self.row = (self.row + usize::from(lines)).min(self.rows.saturating_sub(1));
            }
            Command::CursorForward(columns) => {
                self.column =
                    (self.column + usize::from(columns)).min(self.columns.saturating_sub(1));
            }
            Command::CursorBack(columns) => {
                self.column = self.column.saturating_sub(usize::from(columns));
            }
            Command::CursorColumn(column) => {
                self.column =
                    usize::from(column.saturating_sub(1)).min(self.columns.saturating_sub(1));
            }
            Command::CursorRow(row) => {
                self.row = usize::from(row.saturating_sub(1)).min(self.rows.saturating_sub(1));
            }
            Command::EraseDisplay(mode) => self.erase_display(mode),
            Command::EraseLine(mode) => self.erase_line(mode),
        }
        self.wrap();
    }

    fn erase_line(&mut self, mode: u16) {
        let blank = self.blank();
        let grid = grid();
        match mode {
            0 => {
                for cell in grid[self.row]
                    .iter_mut()
                    .take(self.columns)
                    .skip(self.column)
                {
                    *cell = blank;
                }
            }
            1 => {
                let end = self
                    .column
                    .min(self.columns.saturating_sub(1))
                    .saturating_add(1)
                    .min(MAX_COLUMNS);
                for cell in grid[self.row].iter_mut().take(end) {
                    *cell = blank;
                }
            }
            _ => grid[self.row] = [blank; MAX_COLUMNS],
        }
    }

    fn erase_display(&mut self, mode: u16) {
        let blank = self.blank();
        let grid = grid();
        match mode {
            0 => {
                for cell in grid[self.row]
                    .iter_mut()
                    .take(self.columns)
                    .skip(self.column)
                {
                    *cell = blank;
                }
                for row in grid.iter_mut().skip(self.row.saturating_add(1)) {
                    row.fill(blank);
                }
            }
            1 => {
                for row in grid.iter_mut().take(self.row.min(MAX_ROWS)) {
                    row.fill(blank);
                }
                let end = self
                    .column
                    .min(self.columns.saturating_sub(1))
                    .saturating_add(1)
                    .min(MAX_COLUMNS);
                for cell in grid[self.row].iter_mut().take(end) {
                    *cell = blank;
                }
            }
            _ => grid.fill([blank; MAX_COLUMNS]),
        }
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
        if self.column + width > self.columns {
            self.column = 0;
            self.row += 1;
            self.wrap();
        }
        if self.row >= MAX_ROWS || self.column >= MAX_COLUMNS {
            return;
        }
        let grid = grid();
        grid[self.row][self.column] = Cell {
            character,
            style: self.style,
        };
        if width > 1 && self.column + 1 < MAX_COLUMNS {
            grid[self.row][self.column + 1] = Cell {
                character: '\0',
                style: self.style,
            };
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

    fn blank(&self) -> Cell {
        Cell {
            character: ' ',
            style: self.style,
        }
    }

    fn backspace(&mut self) {
        if self.column == 0 {
            return;
        }
        let grid = grid();
        self.column -= 1;
        if grid[self.row][self.column].character == '\0' && self.column > 0 {
            grid[self.row][self.column] = self.blank();
            self.column -= 1;
        }
        grid[self.row][self.column] = self.blank();
    }

    fn wrap(&mut self) {
        let grid = grid();
        while self.row >= self.rows {
            let rows = self.rows.max(1);
            grid.copy_within(1..rows, 0);
            grid[rows - 1] = [self.blank(); MAX_COLUMNS];
            self.row = rows - 1;
        }
    }

    fn view(&self) -> render::View {
        render::View {
            width: self.width,
            height: self.height,
            columns: self.columns,
            rows: self.rows,
        }
    }

    /// 重绘整个网格与光标，并提交合成。
    fn refresh(&mut self) {
        // SAFETY: 共享表面已映射，长度为 self.width * self.height。
        let frame = unsafe { Surface::from_raw(self.pixels, self.width, self.height, self.width) };
        render::refresh(
            &frame,
            grid(),
            self.row,
            self.column,
            self.font,
            self.view(),
        );
        let _ = send(self.display, SURFACE_COMMIT, self.surface);
    }

    /// 解除共享表面映射，准备重建窗口。
    pub(crate) fn detach(&self) {
        let _ = region_unmap(self.surface_region);
    }

    /// 按新的窗口内容尺寸重建表面并重排网格。
    pub(crate) fn resize(&mut self, width: usize, height: usize) {
        if width == 0 || height == 0 {
            return;
        }
        let columns = (width / CELL_WIDTH).clamp(1, MAX_COLUMNS);
        let rows = (height / CELL_HEIGHT).clamp(1, MAX_ROWS);
        let bytes = width * height * 4;
        let Some((handle, region, virt)) = map_new(CAPABILITY_SURFACE, bytes) else {
            write(b"hk-server-console: resize frame failed\n");
            return;
        };
        if send_handle(
            self.display,
            WINDOW_RESIZE,
            pack_window_size(self.window, width as u32, height as u32),
            handle,
        ) == u64::MAX
        {
            let _ = close_handle(handle);
            write(b"hk-server-console: resize rejected\n");
            return;
        }
        let old_region = self.surface_region;
        self.width = width;
        self.height = height;
        self.surface_virt = virt;
        self.surface_region = region;
        self.pixels = virt as *mut u32;
        self.columns = columns;
        self.rows = rows;
        self.row = self.row.min(rows.saturating_sub(1));
        self.column = self.column.min(columns.saturating_sub(1));
        let _ = region_unmap(old_region);
        self.refresh();
    }

    /// 清空终端并重绘。
    pub(crate) fn clear(&mut self) {
        reset_grid();
        self.style = Style::DEFAULT;
        self.decoder = Utf8::new();
        self.ansi = Ansi::new();
        self.column = 0;
        self.row = 0;
        self.refresh();
    }

    /// 返回窗口编号。
    pub(crate) fn window(&self) -> u64 {
        self.window
    }

    /// 返回 Display 端点。
    pub(crate) fn display(&self) -> u64 {
        self.display
    }
}

/// 尝试连接 Display 并创建终端窗口。
pub(crate) fn attach_terminal(endpoint: u64) -> Option<Terminal> {
    let display = service_endpoint(DISPLAY_SERVICE);
    if display == 0 {
        return None;
    }
    let (handle, surface_region, surface_virt) = map_new(CAPABILITY_SURFACE, WIDTH * HEIGHT * 4)?;
    if send_handle(
        display,
        SURFACE_CREATE,
        pack_size(WIDTH as u32, HEIGHT as u32),
        handle,
    ) == u64::MAX
    {
        return None;
    }
    let surface = wait_reply(endpoint, SURFACE_CREATED)?;
    let _ = send(display, WINDOW_CREATE, surface);
    let window = wait_reply(endpoint, WINDOW_CREATED)?;
    reset_grid();
    Some(Terminal {
        display,
        surface,
        window,
        surface_virt,
        surface_region,
        pixels: surface_virt as *mut u32,
        width: WIDTH,
        height: HEIGHT,
        columns: COLUMNS,
        rows: ROWS,
        column: 0,
        row: 0,
        font: default_monospace(),
        decoder: Utf8::new(),
        style: Style::DEFAULT,
        ansi: Ansi::new(),
        sgr_seen: false,
        utf8_seen: false,
        cjk_seen: false,
    })
}

/// 等待指定标签的回复；显示服务启动期间可能已有其他消息到达。
fn wait_reply(endpoint: u64, expected: u64) -> Option<u64> {
    let mut payload = [0_u8; 8];
    for _ in 0..4096 {
        let (label, word, _sender, _size) = recv_data_try(endpoint, &mut payload);
        if label == expected {
            return (word != 0).then_some(word);
        }
        let _ = yield_now();
    }
    None
}
