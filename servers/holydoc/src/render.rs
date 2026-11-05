//! 文档绘制：纸张、代码块、标题、链接、选中高亮与底部提示栏。

use alloc::format;

use lovyangfx_fonts::Font;

use crate::document::{
    CONTENT_HEIGHT, DocView, FOOTER_HEIGHT, HEIGHT, LINE_HEIGHT, MARGIN, Style, WIDTH,
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
pub(crate) fn render(view: &DocView, font: &Font<'static>, pixels: *mut u32) {
    fill_rect(pixels, 0, 0, WIDTH, HEIGHT, PAPER, HEIGHT);
    fill_rect(
        pixels,
        0,
        CONTENT_HEIGHT as isize,
        WIDTH,
        FOOTER_HEIGHT,
        FOOTER_BG,
        HEIGHT,
    );

    for (index, line) in view.lines.iter().enumerate() {
        let y = MARGIN as isize + (index * LINE_HEIGHT) as isize - view.scroll as isize;
        if y + 18 < 0 || y >= CONTENT_HEIGHT as isize {
            continue;
        }
        if line.code {
            let first = y.max(0) as usize;
            let last = (y + LINE_HEIGHT as isize).clamp(0, CONTENT_HEIGHT as isize) as usize;
            for row in first..last {
                // SAFETY: 行已裁剪到正文区，代码区在左右留白之间。
                unsafe {
                    core::slice::from_raw_parts_mut(
                        pixels.add(row * WIDTH + MARGIN),
                        WIDTH - MARGIN * 2,
                    )
                    .fill(CODE_BG);
                }
            }
        }
        let mut pen = 0;
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
                let width = char_width(character);
                if selected {
                    fill_rect(pixels, pen, y, width, LINE_HEIGHT, SELECT, CONTENT_HEIGHT);
                }
                if run.target.is_some() {
                    let underline = y + 17;
                    if underline >= 0 && underline < CONTENT_HEIGHT as isize {
                        for x in pen..pen + width {
                            if x < WIDTH {
                                // SAFETY: 坐标已裁剪。
                                unsafe {
                                    pixels
                                        .add(underline as usize * WIDTH + x)
                                        .write_volatile(LINK);
                                }
                            }
                        }
                    }
                }
                if let Some(glyph) = font.glyph(character) {
                    glyph.for_each_pixel(|px, py| {
                        let screen_x = pen as i32 + px;
                        let screen_y = y as i32 + py;
                        if screen_x >= 0
                            && screen_y >= 0
                            && (screen_x as usize) < WIDTH
                            && (screen_y as usize) < CONTENT_HEIGHT
                        {
                            // SAFETY: 坐标已裁剪。
                            unsafe {
                                pixels
                                    .add(screen_y as usize * WIDTH + screen_x as usize)
                                    .write_volatile(color);
                            }
                        }
                    });
                }
                pen += width;
            }
        }
    }

    let text_y = CONTENT_HEIGHT + (FOOTER_HEIGHT.saturating_sub(16)) / 2;
    draw_text(font, pixels, MARGIN, text_y, HINT, FOOTER_INK);
    let percent = format!("{}%", view.scroll_percent());
    let width: usize = percent.chars().map(char_width).sum();
    draw_text(
        font,
        pixels,
        WIDTH.saturating_sub(MARGIN + width),
        text_y,
        &percent,
        FOOTER_INK,
    );
}

/// 用单色填充矩形，行裁剪到 `limit` 上限。
fn fill_rect(
    pixels: *mut u32,
    x: usize,
    y: isize,
    width: usize,
    height: usize,
    color: u32,
    limit: usize,
) {
    let left = x.min(WIDTH);
    let right = x.saturating_add(width).min(WIDTH);
    if left >= right {
        return;
    }
    for row in 0..height {
        let screen = y + row as isize;
        if screen < 0 || screen as usize >= limit {
            continue;
        }
        // SAFETY: 行与列都已裁剪到表面范围内。
        unsafe {
            core::slice::from_raw_parts_mut(
                pixels.add(screen as usize * WIDTH + left),
                right - left,
            )
            .fill(color);
        }
    }
}

/// 在指定位置绘制文本。
fn draw_text(font: &Font<'static>, pixels: *mut u32, x: usize, y: usize, text: &str, color: u32) {
    let mut pen = x;
    for character in text.chars() {
        if let Some(glyph) = font.glyph(character) {
            glyph.for_each_pixel(|px, py| {
                let screen_x = pen as i32 + px;
                let screen_y = y as i32 + py;
                if screen_x >= 0
                    && screen_y >= 0
                    && (screen_x as usize) < WIDTH
                    && (screen_y as usize) < HEIGHT
                {
                    // SAFETY: 坐标已裁剪。
                    unsafe {
                        pixels
                            .add(screen_y as usize * WIDTH + screen_x as usize)
                            .write_volatile(color);
                    }
                }
            });
        }
        pen += char_width(character);
    }
}
