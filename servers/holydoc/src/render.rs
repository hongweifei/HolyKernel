//! 文档绘制：纸张、代码块、标题、链接、选中高亮与底部提示栏。

use alloc::format;

use hk_canvas::{Rect, Surface, text_width};

use crate::document::{
    CONTENT_HEIGHT, DocView, FOOTER_HEIGHT, HEIGHT, LINE_HEIGHT, MARGIN, Style, WIDTH, font_for,
};
use crate::markup::char_width;

/// 纸张背景色。
const PAPER: u32 = 0x00F6_F1E7;
/// 正文颜色。
const INK: u32 = 0x0024_2A30;
/// 标题颜色。
const HEADER: u32 = 0x0040_6080;
/// 链接颜色。
const LINK: u32 = 0x0030_6090;
/// 代码背景色。
const CODE_BG: u32 = 0x00E8_E2D6;
/// 代码文字颜色。
const CODE_INK: u32 = 0x0060_4030;
/// 选中链接的背景色。
const SELECT: u32 = 0x00C8_DCF0;
/// 底部提示栏背景色。
const FOOTER_BG: u32 = 0x00E4_DCCD;
/// 底部提示栏文字颜色。
const FOOTER_INK: u32 = 0x0050_5860;

/// 底部快捷键提示。
const HINT: &str =
    "j/k 滚动 | u/d 半页 | n/N 链接 | Enter 打开 | [] 历史 | i 索引 | t 标签 | q 退出";

/// 把文档绘制到共享表面。
pub(crate) fn render(view: &DocView, surface: &Surface) {
    let content_height = CONTENT_HEIGHT as i32;
    let content = surface.clipped(Rect::new(0, 0, WIDTH as i32, content_height));
    surface.fill(Rect::new(0, 0, WIDTH as i32, HEIGHT as i32), PAPER);
    surface.fill(
        Rect::new(0, content_height, WIDTH as i32, FOOTER_HEIGHT as i32),
        FOOTER_BG,
    );

    for (index, line) in view.lines.iter().enumerate() {
        let y = MARGIN as i32 + (index * LINE_HEIGHT) as i32 - view.scroll as i32;
        if y + 18 < 0 || y >= content_height {
            continue;
        }
        let font = font_for(line.code);
        if line.code {
            let top = y.max(0);
            let bottom = (y + LINE_HEIGHT as i32).min(content_height);
            content.fill(
                Rect::new(
                    MARGIN as i32,
                    top,
                    (WIDTH - MARGIN * 2) as i32,
                    bottom - top,
                ),
                CODE_BG,
            );
        }
        let mut pen = MARGIN as i32;
        for run in &line.runs {
            let color = match run.style {
                Style::Title | Style::Heading => HEADER,
                Style::Link => LINK,
                Style::Code => CODE_INK,
                _ => INK,
            };
            let selected =
                run.target.is_some() && run.target.as_deref() == view.selected.as_deref();
            for character in run.text.chars() {
                let width = char_width(font, character) as i32;
                if selected {
                    content.fill(Rect::new(pen, y, width, LINE_HEIGHT as i32), SELECT);
                }
                if run.target.is_some() {
                    content.fill(Rect::new(pen, y + 17, width, 1), LINK);
                }
                content.draw_glyph(pen, y, color, character, font);
                pen += width;
            }
        }
    }

    let footer_font = font_for(false);
    let text_y = (CONTENT_HEIGHT + (FOOTER_HEIGHT.saturating_sub(16)) / 2) as i32;
    let mut pen = MARGIN as i32;
    surface.draw_text(&mut pen, text_y, HINT, FOOTER_INK, footer_font);
    let percent = format!("{}%", view.scroll_percent());
    let mut pen = WIDTH as i32 - MARGIN as i32 - text_width(footer_font, &percent);
    surface.draw_text(&mut pen, text_y, &percent, FOOTER_INK, footer_font);
}
