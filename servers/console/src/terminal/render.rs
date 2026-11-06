//! 终端表面绘制：单元格背景、字形、属性与光标。

use hk_canvas::{Rect, Surface, mix};
use hk_font::Font;

use super::style::{BACKGROUND, CURSOR, FLAG_BOLD, FLAG_DIM, FLAG_REVERSE, FLAG_UNDERLINE};
use super::{CELL_HEIGHT, CELL_WIDTH, Grid};

/// 终端可视区域。
#[derive(Clone, Copy)]
pub(crate) struct View {
    /// 表面宽度（像素）。
    pub(crate) width: usize,
    /// 表面高度（像素）。
    pub(crate) height: usize,
    /// 可视列数。
    pub(crate) columns: usize,
    /// 可视行数。
    pub(crate) rows: usize,
}

/// 重绘整个网格与光标。
pub(crate) fn refresh(
    surface: &Surface,
    grid: &Grid,
    cursor_row: usize,
    cursor_column: usize,
    font: &Font,
    view: View,
) {
    let width = view.width.min(i32::MAX as usize) as i32;
    let height = view.height.min(i32::MAX as usize) as i32;
    surface.fill(Rect::new(0, 0, width, height), BACKGROUND);
    // 先统一铺背景，避免后一个单元格的背景覆盖宽字符的右半部分。
    let rows = view.rows.min(grid.len());
    for (row, line) in grid.iter().enumerate().take(rows) {
        let columns = view.columns.min(line.len());
        for (column, cell) in line.iter().enumerate().take(columns) {
            let background = if cell.style.flags & FLAG_REVERSE != 0 {
                cell.style.foreground
            } else {
                cell.style.background
            };
            surface.fill(
                Rect::new(
                    (column * CELL_WIDTH) as i32,
                    (row * CELL_HEIGHT) as i32,
                    CELL_WIDTH as i32,
                    CELL_HEIGHT as i32,
                ),
                background,
            );
        }
    }
    for (row, line) in grid.iter().enumerate().take(rows) {
        let columns = view.columns.min(line.len());
        for (column, cell) in line.iter().enumerate().take(columns) {
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
            if !font.has_glyph(cell.character) {
                continue;
            }
            if cell.style.flags & FLAG_DIM != 0 {
                foreground = mix(foreground, background, 128);
            }
            let wide = font.advance(cell.character).max(0) as usize > CELL_WIDTH;
            let cell_pixels = if wide { CELL_WIDTH * 2 } else { CELL_WIDTH };
            let cell_surface = surface.clipped(Rect::new(
                origin_x as i32,
                origin_y as i32,
                cell_pixels as i32,
                CELL_HEIGHT as i32,
            ));
            cell_surface.draw_glyph(
                origin_x as i32,
                origin_y as i32,
                foreground,
                cell.character,
                font,
            );
            if cell.style.flags & FLAG_BOLD != 0 {
                cell_surface.draw_glyph(
                    origin_x as i32 + 1,
                    origin_y as i32,
                    foreground,
                    cell.character,
                    font,
                );
            }
            if cell.style.flags & FLAG_UNDERLINE != 0 {
                let y = origin_y + CELL_HEIGHT - 2;
                surface.fill(
                    Rect::new(origin_x as i32, y as i32, cell_pixels as i32, 1),
                    foreground,
                );
            }
        }
    }
    if cursor_row < view.rows && cursor_column < view.columns {
        let cursor_y = cursor_row * CELL_HEIGHT + CELL_HEIGHT - 2;
        let cursor_x = cursor_column * CELL_WIDTH;
        surface.fill(
            Rect::new(cursor_x as i32, cursor_y as i32, CELL_WIDTH as i32, 2),
            CURSOR,
        );
    }
}
