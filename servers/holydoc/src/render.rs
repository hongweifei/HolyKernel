//! 文档绘制：纸张、代码块、标题与链接文字。

use lovyangfx_fonts::Font;

use crate::document::{DocView, HEIGHT, LINE_HEIGHT, MARGIN, Style, WIDTH};
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

/// 把文档绘制到共享表面。
pub(crate) fn render(view: &DocView, font: &Font<'static>, pixels: *mut u32) {
    for y in 0..HEIGHT {
        // SAFETY: 共享表面已按 WIDTH×HEIGHT 映射，行范围在表面内。
        unsafe {
            core::slice::from_raw_parts_mut(pixels.add(y * WIDTH), WIDTH).fill(PAPER);
        }
    }
    for (index, line) in view.lines.iter().enumerate() {
        let y = MARGIN as isize + (index * LINE_HEIGHT) as isize - view.scroll as isize;
        if y + 18 < 0 || y >= HEIGHT as isize {
            continue;
        }
        if line.code {
            let first = y.max(0) as usize;
            let last = (y + LINE_HEIGHT as isize).clamp(0, HEIGHT as isize) as usize;
            for row in first..last {
                // SAFETY: 行已裁剪到表面内，代码区在左右留白之间。
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
            for character in run.text.chars() {
                let width = char_width(character);
                if run.target.is_some() {
                    let underline = y + 17;
                    if underline >= 0 && underline < HEIGHT as isize {
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
                pen += width;
            }
        }
    }
}
