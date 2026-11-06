//! 文本绘制：字形来源与字符串排版。

use lovyangfx_fonts::Font as BitmapFont;

use crate::surface::Surface;

/// 字形来源。
///
/// 实现者提供字符的前进宽度与覆盖像素。像素坐标相对单元格左上角，与
/// [`Surface::draw_glyph`] 传入的绘制原点相加即为表面坐标。
pub trait GlyphSource {
    /// 字符的水平前进宽度（像素）。
    fn advance(&self, character: char) -> i32;

    /// 遍历字形的覆盖像素。
    fn for_each_pixel(&self, character: char, draw: impl FnMut(i32, i32));
}

impl GlyphSource for BitmapFont<'_> {
    fn advance(&self, character: char) -> i32 {
        match self.glyph(character) {
            Some(glyph) => glyph.metric().xadvance.max(0),
            None => self.max_width(),
        }
    }

    fn for_each_pixel(&self, character: char, draw: impl FnMut(i32, i32)) {
        if let Some(glyph) = self.glyph(character) {
            glyph.for_each_pixel(draw);
        }
    }
}

impl Surface<'_> {
    /// 在 `(x, y)` 绘制一个字形。
    pub fn draw_glyph(&self, x: i32, y: i32, color: u32, character: char, font: &impl GlyphSource) {
        font.for_each_pixel(character, |offset_x, offset_y| {
            self.put(
                x.saturating_add(offset_x),
                y.saturating_add(offset_y),
                color,
            );
        });
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
