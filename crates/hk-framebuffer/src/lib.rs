//! 帧缓冲文本控制台。
//!
//! 用 16 px 点阵中文字体（eFont CN，GB2312 常用汉字 + ASCII）把文本画到 Limine
//! 提供的线性帧缓冲。光标按字形实际宽度推进：ASCII 半宽、汉字全宽；写满一屏后
//! 整体上移一行。帧缓冲位于 HHDM 中，基址为 `hhdm + 物理地址`。

#![no_std]

extern crate alloc;

use alloc::vec::Vec;

use hk_hal::{BootInfo, Console, FramebufferInfo, PixelFormat};
use lovyangfx_fonts::Font;
use lovyangfx_fonts::fonts::efont_cn_16;
use spin::Mutex;

const FOREGROUND: u32 = 0x00D0_D0D0;
const BACKGROUND: u32 = 0x0000_0000;
const BYTES_PER_PIXEL: usize = 4;

struct Cursor {
    x: usize,
    row: usize,
    advances: Vec<usize>,
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
    pub fn from_boot_info(boot: &BootInfo) -> Option<Self> {
        let framebuffer = boot.framebuffer?;
        let direct_map = boot.direct_map?;
        let mapped = direct_map.virtual_address(framebuffer.phys)?;
        let direct_map_base = mapped.get().checked_sub(framebuffer.phys.get())?;
        Some(Self::new(&framebuffer, direct_map_base))
    }

    /// 以帧缓冲描述与 direct map 基址创建控制台。
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
            cursor: Mutex::new(Cursor {
                x: 0,
                row: 0,
                advances: Vec::new(),
            }),
        }
    }

    /// 返回以全角为单位的字符列数。
    pub fn columns(&self) -> usize {
        self.columns
    }

    /// 返回字符行数。
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// 返回全角单元格宽度，单位为像素。
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
        cursor.x = 0;
        cursor.row = 0;
        cursor.advances.clear();
    }

    /// 擦除光标前一个字符并把光标左移。
    pub fn backspace(&self) {
        let mut cursor = self.cursor.lock();
        let Some(advance) = cursor.advances.pop() else {
            return;
        };
        cursor.x = cursor.x.saturating_sub(advance);
        let y = cursor.row * self.line_height;
        self.fill_rect(cursor.x, y, advance, self.line_height, BACKGROUND);
    }

    fn advance(&self, character: char) -> usize {
        let character = if self.font.has_glyph(character) {
            character
        } else {
            '?'
        };
        self.font.glyph(character).map_or(self.cell_width, |glyph| {
            glyph.metric().xadvance.max(0) as usize
        })
    }

    fn draw_char(&self, character: char, x: usize, row: usize) -> usize {
        let character = if self.font.has_glyph(character) {
            character
        } else {
            '?'
        };
        let Some(glyph) = self.font.glyph(character) else {
            return self.cell_width;
        };

        let metric = glyph.metric();
        let advance = metric.xadvance.max(0) as usize;
        let y = row * self.line_height;
        self.fill_rect(x, y, advance, self.line_height, BACKGROUND);

        let origin_x = x as i32;
        let origin_y = y as i32;
        glyph.for_each_pixel(|glyph_x, glyph_y| {
            let pixel_x = origin_x + glyph_x;
            let pixel_y = origin_y + glyph_y;
            if pixel_x >= 0 && pixel_y >= 0 {
                self.write_pixel(pixel_x as usize, pixel_y as usize, FOREGROUND);
            }
        });
        advance
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
        cursor.x = 0;
        cursor.row += 1;
        cursor.advances.clear();
        if cursor.row >= self.rows {
            self.scroll(cursor);
        }
    }

    fn write_char(&self, cursor: &mut Cursor, character: char) {
        let advance = self.advance(character);
        if cursor.x + advance > self.width {
            self.newline(cursor);
        }
        let advance = self.draw_char(character, cursor.x, cursor.row);
        cursor.x += advance;
        cursor.advances.push(advance);
    }
}

impl Console for FramebufferConsole {
    fn write_str(&self, text: &str) {
        let mut cursor = self.cursor.lock();

        for character in text.chars() {
            match character {
                '\n' => self.newline(&mut cursor),
                '\r' => {
                    cursor.x = 0;
                    cursor.advances.clear();
                }
                '\t' => {
                    for _ in 0..4 {
                        self.write_char(&mut cursor, ' ');
                    }
                }
                _ => self.write_char(&mut cursor, character),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hk_hal::PhysAddr;

    fn console() -> FramebufferConsole {
        let info = FramebufferInfo {
            phys: PhysAddr::new(0),
            width: 1280,
            height: 800,
            stride: 5120,
            format: PixelFormat::Bgr32,
        };
        FramebufferConsole::new(&info, 0)
    }

    #[test]
    fn advances_by_glyph_width() {
        let console = console();
        assert_eq!(console.advance('A'), 8);
        assert_eq!(console.advance(' '), 8);
        assert_eq!(console.advance('?'), 8);
        assert_eq!(console.advance('你'), 16);
    }

    #[test]
    fn columns_use_full_width_cells() {
        let console = console();
        assert_eq!(console.columns(), 80);
        assert_eq!(console.rows(), 36);
    }
}
