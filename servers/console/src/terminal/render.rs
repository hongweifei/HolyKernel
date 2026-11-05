//! 终端表面绘制：单元格背景、字形、属性与光标。

use lovyangfx_fonts::Font;

use super::style::{BACKGROUND, CURSOR, FLAG_BOLD, FLAG_DIM, FLAG_REVERSE, FLAG_UNDERLINE};
use super::{CELL_HEIGHT, CELL_WIDTH, Grid, HEIGHT, WIDTH};

/// 重绘整个网格与光标。
pub(crate) fn refresh(
    pixels: *mut u32,
    grid: &Grid,
    cursor_row: usize,
    cursor_column: usize,
    font: &Font<'static>,
) {
    for y in 0..HEIGHT {
        // SAFETY: 共享表面按 WIDTH×HEIGHT 映射，行范围在表面内。
        unsafe {
            core::slice::from_raw_parts_mut(pixels.add(y * WIDTH), WIDTH).fill(BACKGROUND);
        }
    }
    // 先统一铺背景，避免后一个单元格的背景覆盖宽字符的右半部分。
    for (row, line) in grid.iter().enumerate() {
        for (column, cell) in line.iter().enumerate() {
            let background = if cell.style.flags & FLAG_REVERSE != 0 {
                cell.style.foreground
            } else {
                cell.style.background
            };
            fill(
                pixels,
                column * CELL_WIDTH,
                row * CELL_HEIGHT,
                CELL_WIDTH,
                CELL_HEIGHT,
                background,
            );
        }
    }
    for (row, line) in grid.iter().enumerate() {
        for (column, cell) in line.iter().enumerate() {
            if cell.character == ' ' || cell.character == '\0' {
                continue;
            }
            let origin_x = column * CELL_WIDTH;
            let origin_y = row * CELL_HEIGHT;
            let mut foreground = cell.style.foreground;
            let mut background = cell.style.background;
            if cell.style.flags & FLAG_REVERSE != 0 {
                core::mem::swap(&mut foreground, &mut background);
            }
            let Some(glyph) = font.glyph(cell.character) else {
                continue;
            };
            if cell.style.flags & FLAG_DIM != 0 {
                foreground = blend(foreground, background, 128);
            }
            let wide = glyph.metric().xadvance.max(0) as usize > CELL_WIDTH;
            let cell_pixels = if wide { CELL_WIDTH * 2 } else { CELL_WIDTH };
            let draw = |x_offset: usize| {
                glyph.for_each_pixel(|x, y| {
                    let pixel_x = origin_x as i32 + x + x_offset as i32;
                    let pixel_y = origin_y as i32 + y;
                    if pixel_x >= origin_x as i32
                        && pixel_x < (origin_x + cell_pixels) as i32
                        && pixel_y >= 0
                    {
                        put_pixel(pixels, pixel_x as usize, pixel_y as usize, foreground);
                    }
                });
            };
            draw(0);
            if cell.style.flags & FLAG_BOLD != 0 {
                draw(1);
            }
            if cell.style.flags & FLAG_UNDERLINE != 0 {
                let y = origin_y + CELL_HEIGHT - 2;
                fill(pixels, origin_x, y, cell_pixels, 1, foreground);
            }
        }
    }
    let cursor_y = cursor_row * CELL_HEIGHT + CELL_HEIGHT - 2;
    let cursor_x = cursor_column * CELL_WIDTH;
    fill(pixels, cursor_x, cursor_y, CELL_WIDTH, 2, CURSOR);
}

fn fill(pixels: *mut u32, x: usize, y: usize, width: usize, height: usize, color: u32) {
    for row in y..y.saturating_add(height).min(HEIGHT) {
        for column in x..x.saturating_add(width).min(WIDTH) {
            // SAFETY: 坐标已裁剪到表面内。
            unsafe {
                pixels.add(row * WIDTH + column).write_volatile(color);
            }
        }
    }
}

fn put_pixel(pixels: *mut u32, x: usize, y: usize, color: u32) {
    if x >= WIDTH || y >= HEIGHT {
        return;
    }
    // SAFETY: 坐标已裁剪到表面内。
    unsafe {
        pixels.add(y * WIDTH + x).write_volatile(color);
    }
}

fn blend(from: u32, to: u32, ratio: u32) -> u32 {
    let inverse = 255 - ratio;
    let red = (((from >> 16) & 0xFF) * inverse + ((to >> 16) & 0xFF) * ratio) / 255;
    let green = (((from >> 8) & 0xFF) * inverse + ((to >> 8) & 0xFF) * ratio) / 255;
    let blue = ((from & 0xFF) * inverse + (to & 0xFF) * ratio) / 255;
    (red << 16) | (green << 8) | blue
}
