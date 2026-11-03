//! 帧缓冲文本控制台。
//!
//! 用 16 px 点阵中文字体（eFont CN，GB2312 常用汉字 + ASCII）把文本画到 Limine
//! 提供的线性帧缓冲。每个字符占一个固定单元格，写满一屏后整体上移一行。帧缓冲
//! 位于 HHDM 中，基址为 `hhdm + 物理地址`。

use hk_hal::{Console, FramebufferInfo, PixelFormat};
use lovyangfx_fonts::Font;
use lovyangfx_fonts::fonts::efont_cn_16;
use spin::Mutex;

const FOREGROUND: u32 = 0x00D0_D0D0;
const BACKGROUND: u32 = 0x0000_0000;
const BYTES_PER_PIXEL: usize = 4;

struct Cursor {
    column: usize,
    row: usize,
}

/// 帧缓冲文本控制台。
pub struct FramebufferConsole {
    base: *mut u8,
    width: usize,
    height: usize,
    stride: usize,
    format: PixelFormat,
    font: Font<'static>,
    cell_width: usize,
    cell_height: usize,
    line_height: usize,
    columns: usize,
    rows: usize,
    cursor: Mutex<Cursor>,
}

// SAFETY: 帧缓冲是固定映射的设备内存，所有写入都经 `cursor` 锁串行化。
unsafe impl Send for FramebufferConsole {}
// SAFETY: 同上；`base` 在控制台生命周期内不变。
unsafe impl Sync for FramebufferConsole {}

impl FramebufferConsole {
    /// 以启动信息中的帧缓冲描述创建控制台。
    pub fn new(info: &FramebufferInfo, direct_map_base: u64) -> Self {
        let font = efont_cn_16();
        let cell_width = font.max_width() as usize;
        let cell_height = font.max_height() as usize;
        // 行距留出行间空白，中文满格字形不会与相邻行相贴。
        let line_height = cell_height + 6;

        Self {
            base: (direct_map_base + info.phys.get()) as *mut u8,
            width: info.width as usize,
            height: info.height as usize,
            stride: info.stride as usize,
            format: info.format,
            font,
            cell_width,
            cell_height,
            line_height,
            columns: info.width as usize / cell_width,
            rows: info.height as usize / line_height,
            cursor: Mutex::new(Cursor { column: 0, row: 0 }),
        }
    }

    /// 返回字符列数。
    pub fn columns(&self) -> usize {
        self.columns
    }

    /// 返回字符行数。
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// 返回单元格宽度，单位为像素。
    pub fn cell_width(&self) -> usize {
        self.cell_width
    }

    /// 返回单元格高度，单位为像素。
    pub fn cell_height(&self) -> usize {
        self.cell_height
    }

    /// 清屏并把光标移到左上角。
    pub fn clear(&self) {
        let mut cursor = self.cursor.lock();
        self.fill_rect(0, 0, self.width, self.height, BACKGROUND);
        cursor.column = 0;
        cursor.row = 0;
    }

    /// 擦除光标前一个字符并把光标左移。
    pub fn backspace(&self) {
        let mut cursor = self.cursor.lock();
        if cursor.column == 0 {
            if cursor.row == 0 {
                return;
            }
            cursor.row -= 1;
            cursor.column = self.columns - 1;
        } else {
            cursor.column -= 1;
        }
        self.draw_char(' ', cursor.column, cursor.row);
    }

    fn draw_char(&self, character: char, column: usize, row: usize) {
        let origin_x = (column * self.cell_width) as i32;
        let origin_y = (row * self.line_height) as i32;
        self.fill_rect(
            column * self.cell_width,
            row * self.line_height,
            self.cell_width,
            self.line_height,
            BACKGROUND,
        );

        let character = if self.font.has_glyph(character) {
            character
        } else {
            '?'
        };
        let Some(glyph) = self.font.glyph(character) else {
            return;
        };

        glyph.for_each_pixel(|x, y| {
            let pixel_x = origin_x + x;
            let pixel_y = origin_y + y;
            if pixel_x >= 0 && pixel_y >= 0 {
                self.write_pixel(pixel_x as usize, pixel_y as usize, FOREGROUND);
            }
        });
    }

    fn write_pixel(&self, x: usize, y: usize, color: u32) {
        if x >= self.width || y >= self.height {
            return;
        }
        let offset = y * self.stride + x * BYTES_PER_PIXEL;
        let bytes = match self.format {
            PixelFormat::Rgb32 => [(color >> 16) as u8, (color >> 8) as u8, color as u8, 0],
            PixelFormat::Bgr32 => [color as u8, (color >> 8) as u8, (color >> 16) as u8, 0],
        };
        // SAFETY: 偏移由已校验的坐标算出，落在帧缓冲范围内；写入限定为 4 字节。
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), self.base.add(offset), BYTES_PER_PIXEL);
        }
    }

    fn fill_rect(&self, x: usize, y: usize, width: usize, height: usize, color: u32) {
        for row in y..y + height {
            for column in x..x + width {
                self.write_pixel(column, row, color);
            }
        }
    }

    fn scroll(&self, cursor: &mut Cursor) {
        let line_bytes = self.stride * self.line_height;
        let total_bytes = self.stride * self.height;
        // SAFETY: 源与目标都在帧缓冲内，区间重叠由 `copy` 处理。
        unsafe {
            core::ptr::copy(
                self.base.add(line_bytes),
                self.base,
                total_bytes - line_bytes,
            );
        }
        self.fill_rect(
            0,
            self.height - self.line_height,
            self.width,
            self.line_height,
            BACKGROUND,
        );
        cursor.row = self.rows - 1;
    }

    fn newline(&self, cursor: &mut Cursor) {
        cursor.column = 0;
        cursor.row += 1;
        if cursor.row >= self.rows {
            self.scroll(cursor);
        }
    }
}

impl Console for FramebufferConsole {
    fn write_str(&self, text: &str) {
        let mut cursor = self.cursor.lock();

        for character in text.chars() {
            match character {
                '\n' => self.newline(&mut cursor),
                '\r' => cursor.column = 0,
                '\t' => {
                    for _ in 0..4 {
                        self.draw_char(' ', cursor.column, cursor.row);
                        cursor.column += 1;
                        if cursor.column >= self.columns {
                            self.newline(&mut cursor);
                        }
                    }
                }
                _ => {
                    self.draw_char(character, cursor.column, cursor.row);
                    cursor.column += 1;
                    if cursor.column >= self.columns {
                        self.newline(&mut cursor);
                    }
                }
            }
        }
    }
}
