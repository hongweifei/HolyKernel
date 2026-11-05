//! 调试窗口绘制：标题、正文换行与底部状态栏。

use alloc::format;

use lovyangfx_fonts::Font;

/// 窗口宽度。
pub(crate) const WIDTH: usize = 640;
/// 窗口高度。
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
pub(crate) fn visible_lines() -> usize {
    (HEIGHT - BODY_TOP - FOOTER_HEIGHT) / LINE_HEIGHT
}

/// 统计换行后的可视行数。
pub(crate) fn line_count(text: &str) -> usize {
    let mut lines = 1;
    let mut x = 0;
    for character in text.chars() {
        if character == '\n' {
            lines += 1;
            x = 0;
            continue;
        }
        let width = char_width(character);
        if x + width > WIDTH - MARGIN * 2 {
            lines += 1;
            x = 0;
        }
        x += width;
    }
    lines
}

/// 绘制页面。
pub(crate) fn render(
    text: &str,
    title: &str,
    scroll: usize,
    font: &Font<'static>,
    pixels: *mut u32,
) {
    fill(pixels, 0, 0, WIDTH, HEIGHT, BACKGROUND);
    fill(
        pixels,
        0,
        HEIGHT - FOOTER_HEIGHT,
        WIDTH,
        FOOTER_HEIGHT,
        FOOTER_BG,
    );

    let mut x = MARGIN as i32;
    draw_text(font, pixels, &mut x, 12, "HolyKernel Debug · ", HEADER);
    draw_text(font, pixels, &mut x, 12, title, HEADER);

    let visible = visible_lines();
    let mut line = 0;
    let mut pen = MARGIN as i32;
    for character in text.chars() {
        if character == '\n' {
            line += 1;
            pen = MARGIN as i32;
            continue;
        }
        let width = char_width(character);
        if pen + width as i32 > (WIDTH - MARGIN) as i32 {
            line += 1;
            pen = MARGIN as i32;
        }
        if line >= scroll + visible {
            break;
        }
        if line >= scroll {
            let y = BODY_TOP + (line - scroll) * LINE_HEIGHT;
            put_char(font, pixels, pen, y as i32, character, FOREGROUND);
        }
        pen += width as i32;
    }

    let mut x = MARGIN as i32;
    let y = (HEIGHT - FOOTER_HEIGHT + 4) as i32;
    draw_text(font, pixels, &mut x, y, HINT, FOOTER_INK);
    let total = line_count(text);
    let position = format!("{}/{}", scroll + 1, total);
    let width: usize = position.chars().map(char_width).sum();
    let mut x = (WIDTH - MARGIN - width) as i32;
    draw_text(font, pixels, &mut x, y, &position, FOOTER_INK);
}

fn char_width(character: char) -> usize {
    if u32::from(character) > 0x7F { 16 } else { 8 }
}

fn put_char(font: &Font<'static>, pixels: *mut u32, x: i32, y: i32, character: char, color: u32) {
    let Some(glyph) = font.glyph(character) else {
        return;
    };
    glyph.for_each_pixel(|px, py| {
        let screen_x = x + px;
        let screen_y = y + py;
        if screen_x >= MARGIN as i32
            && screen_y >= BODY_TOP as i32
            && (screen_x as usize) < WIDTH - MARGIN
            && (screen_y as usize) < HEIGHT - FOOTER_HEIGHT
        {
            // SAFETY: 坐标已裁剪到正文区。
            unsafe {
                pixels
                    .add(screen_y as usize * WIDTH + screen_x as usize)
                    .write_volatile(color);
            }
        }
    });
}

fn draw_text(font: &Font<'static>, pixels: *mut u32, x: &mut i32, y: i32, text: &str, color: u32) {
    for character in text.chars() {
        let Some(glyph) = font.glyph(character) else {
            *x += char_width(character) as i32;
            continue;
        };
        glyph.for_each_pixel(|px, py| {
            let screen_x = *x + px;
            let screen_y = y + py;
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
        *x += char_width(character) as i32;
    }
}

fn fill(pixels: *mut u32, x: usize, y: usize, width: usize, height: usize, color: u32) {
    let left = x.min(WIDTH);
    let right = x.saturating_add(width).min(WIDTH);
    if left >= right {
        return;
    }
    for row in y..y.saturating_add(height).min(HEIGHT) {
        // SAFETY: 行列已裁剪到表面内。
        unsafe {
            core::slice::from_raw_parts_mut(pixels.add(row * WIDTH + left), right - left)
                .fill(color);
        }
    }
}
