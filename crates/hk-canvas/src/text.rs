//! 文本绘制：字形覆盖率位图、字符串排版与像素落位。

use crate::surface::{Rect, Surface};

/// 连续全覆盖区间改用矩形填充的最短长度；更短的区间逐像素写入更划算。
const FILL_MIN_RUN: usize = 4;

/// 字形覆盖率位图。
///
/// 坐标以绘制原点为基准：像素 `(left + column, top + row)` 的覆盖率为
/// `coverage[row * stride + column]`，0 表示完全透明，255 表示完全覆盖。
pub struct GlyphMask<'a> {
    /// 位图宽度（像素）。
    pub width: usize,
    /// 位图高度（像素）。
    pub height: usize,
    /// 位图左边相对绘制原点的偏移。
    pub left: i32,
    /// 位图顶边相对绘制原点的偏移。
    pub top: i32,
    /// 覆盖率每行的元素数。
    pub stride: usize,
    /// 覆盖率数据，按行连续存放。
    pub coverage: &'a [u8],
}

impl GlyphMask<'_> {
    /// 返回第 `row` 行的覆盖率；行超出位图高度时返回 `None`。
    pub fn row(&self, row: usize) -> Option<&[u8]> {
        if row >= self.height {
            return None;
        }
        let start = row.checked_mul(self.stride)?;
        self.coverage.get(start..start + self.width)
    }
}

/// 字形来源。
///
/// 实现者提供字符的前进宽度与覆盖率位图。缺失字形的字符不会触发回调，
/// 调用方据此把该字符当作空白处理。
pub trait GlyphSource {
    /// 字符的水平前进宽度（像素）。
    fn advance(&self, character: char) -> i32;

    /// 取出字符的字形位图。
    fn with_glyph(&self, character: char, plot: impl FnMut(&GlyphMask<'_>));
}

impl Surface<'_> {
    /// 绘制覆盖率位图。
    ///
    /// 每行按覆盖率切段：连续全覆盖的区间按矩形填充，半覆盖区间逐像素混合，
    /// 透明像素跳过。位图与表面的交集由裁剪矩形决定。
    pub fn draw_mask(&self, x: i32, y: i32, color: u32, mask: &GlyphMask<'_>) {
        let clip = self.clip();
        for row in 0..mask.height {
            let pixel_y = y.saturating_add(row as i32).saturating_add(mask.top);
            if pixel_y < clip.y || pixel_y >= clip.bottom() {
                continue;
            }
            let coverage = match mask.row(row) {
                Some(coverage) => coverage,
                None => continue,
            };
            let mut index = 0;
            while index < coverage.len() {
                while index < coverage.len() && coverage[index] == 0 {
                    index += 1;
                }
                let opaque_end = index;
                while index < coverage.len() && coverage[index] != 0 {
                    index += 1;
                }
                if opaque_end >= index {
                    break;
                }
                let left = x.saturating_add(mask.left);
                self.draw_run(coverage, opaque_end, index, left, pixel_y, color);
            }
        }
    }

    /// 绘制一段非零覆盖率：连续 255 按行填充，其余逐像素混合。
    fn draw_run(&self, coverage: &[u8], start: usize, end: usize, left: i32, y: i32, color: u32) {
        let mut index = start;
        while index < end {
            if coverage[index] == 255 {
                let begin = index;
                while index < end && coverage[index] == 255 {
                    index += 1;
                }
                let left_pixel = left.saturating_add(begin as i32);
                if index - begin >= FILL_MIN_RUN {
                    self.fill(Rect::new(left_pixel, y, (index - begin) as i32, 1), color);
                } else {
                    for offset in begin..index {
                        self.put(left.saturating_add(offset as i32), y, color);
                    }
                }
            } else {
                let begin = index;
                while index < end && coverage[index] != 255 {
                    index += 1;
                }
                for offset in begin..index {
                    let value = coverage[offset];
                    if value != 0 {
                        self.blend(left.saturating_add(offset as i32), y, color, value);
                    }
                }
            }
        }
    }

    /// 在 `(x, y)` 绘制一个字形。
    pub fn draw_glyph(&self, x: i32, y: i32, color: u32, character: char, font: &impl GlyphSource) {
        font.with_glyph(character, |mask| self.draw_mask(x, y, color, mask));
    }

    /// 从 `pen` 起绘制一串文本；绘制后 `pen` 前进到文本末尾。
    pub fn draw_text(
        &self,
        pen: &mut i32,
        y: i32,
        text: &str,
        color: u32,
        font: &impl GlyphSource,
    ) {
        for character in text.chars() {
            self.draw_glyph(*pen, y, color, character, font);
            *pen = (*pen).saturating_add(font.advance(character));
        }
    }
}

/// 文本按字宽累计的像素宽度。
pub fn text_width(font: &impl GlyphSource, text: &str) -> i32 {
    text.chars().fold(0, |total, character| {
        total.saturating_add(font.advance(character))
    })
}
