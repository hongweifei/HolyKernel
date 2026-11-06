//! 调试窗口绘制：标题、正文换行与底部状态栏。

use alloc::format;

use hk_canvas::{Rect, Surface, text_width};
use hk_font::Font;

/// 初始窗口宽度。
pub(crate) const WIDTH: usize = 640;
/// 初始窗口高度。
pub(crate) const HEIGHT: usize = 480;
/// 行高。
const LINE_HEIGHT: usize = 20;
/// 页面留白。
const MARGIN: usize = 20;
/// 正文顶部。
const BODY_TOP: usize = 44;
/// 底部状态栏高度。
const FOOTER_HEIGHT: usize = 24;

/// 背景色。
const BACKGROUND: u32 = 0x0014_1A20;
/// 正文颜色。
const FOREGROUND: u32 = 0x00C8_E0D0;
/// 标题颜色。
const HEADER: u32 = 0x0080_C0FF;
/// 状态栏背景色。
const FOOTER_BG: u32 = 0x001C_242C;
/// 状态栏文字颜色。
const FOOTER_INK: u32 = 0x0070_8898;
/// 快捷键提示。
const HINT: &str = "1-6 视图 | j/k 滚动 | u/d 半页 | r 刷新 | q 退出";

/// 正文可视行数。
pub(crate) fn visible_lines(height: usize) -> usize {
    height
        .saturating_sub(BODY_TOP + FOOTER_HEIGHT)
        .checked_div(LINE_HEIGHT)
        .unwrap_or(0)
        .max(1)
}

/// 统计换行后的可视行数。
pub(crate) fn line_count(text: &str, width: usize, font: &Font) -> usize {
    let limit = width.saturating_sub(MARGIN.saturating_mul(2));
    let mut lines = 1;
    let mut x = 0;
    for character in text.chars() {
        if character == '\n' {
            lines += 1;
            x = 0;
            continue;
        }
        let advance = font.advance(character) as usize;
        if x + advance > limit && x > 0 {
            lines += 1;
            x = 0;
        }
        x += advance;
    }
    lines
}

/// 绘制页面。
#[allow(clippy::too_many_arguments)]
pub(crate) fn render(
    text: &str,
    title: &str,
    scroll: usize,
    width: usize,
    height: usize,
    font: &Font,
    surface: &Surface,
) {
    let width_i32 = width.min(i32::MAX as usize) as i32;
    let height_i32 = height.min(i32::MAX as usize) as i32;
    let footer_top = height.saturating_sub(FOOTER_HEIGHT);
    surface.fill(Rect::new(0, 0, width_i32, height_i32), BACKGROUND);
    surface.fill(
        Rect::new(
            0,
            footer_top as i32,
            width_i32,
            FOOTER_HEIGHT.min(height) as i32,
        ),
        FOOTER_BG,
    );

    let mut x = MARGIN as i32;
    surface.draw_text(&mut x, 12, "HolyKernel Debug · ", HEADER, font);
    surface.draw_text(&mut x, 12, title, HEADER, font);

    let body_width = width.saturating_sub(MARGIN.saturating_mul(2));
    let body_height = height.saturating_sub(BODY_TOP + FOOTER_HEIGHT);
    let body = surface.clipped(Rect::new(
        MARGIN as i32,
        BODY_TOP as i32,
        body_width.min(i32::MAX as usize) as i32,
        body_height.min(i32::MAX as usize) as i32,
    ));
    let visible = visible_lines(height);
    let text_right = width.saturating_sub(MARGIN);
    let mut line = 0;
    let mut pen = MARGIN as i32;
    for character in text.chars() {
        if character == '\n' {
            line += 1;
            pen = MARGIN as i32;
            continue;
        }
        let advance = font.advance(character);
        if pen + advance > text_right as i32 && pen > MARGIN as i32 {
            line += 1;
            pen = MARGIN as i32;
        }
        if line >= scroll + visible {
            break;
        }
        if line >= scroll {
            let y = BODY_TOP + (line - scroll) * LINE_HEIGHT;
            body.draw_glyph(pen, y as i32, FOREGROUND, character, font);
        }
        pen += advance;
    }

    let mut x = MARGIN as i32;
    let y = footer_top.saturating_add(4) as i32;
    surface.draw_text(&mut x, y, HINT, FOOTER_INK, font);
    let total = line_count(text, width, font);
    let position = format!("{}/{}", scroll + 1, total);
    let mut x = text_right as i32 - text_width(font, &position);
    surface.draw_text(&mut x, y, &position, FOOTER_INK, font);
}
