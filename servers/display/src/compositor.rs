//! Display 合成：桌面、窗口、光标与 damage 区域重绘。

use lovyangfx_fonts::Font;

use crate::theme::theme;
use crate::window::{
    CLOSE_SIZE, MAX_WINDOWS, Screen, TASKBAR_BUTTON_HEIGHT, TASKBAR_BUTTON_WIDTH, TASKBAR_HEIGHT,
    TITLE_HEIGHT, Window, close_button_origin, taskbar_button_top, taskbar_button_x, taskbar_top,
};

/// 光标宽度。
#[cfg(target_os = "none")]
pub(crate) const CURSOR_WIDTH: usize = 8;
/// 光标高度。
#[cfg(target_os = "none")]
pub(crate) const CURSOR_HEIGHT: usize = 12;
/// 光标位图（每行 8 位）。
#[cfg(target_os = "none")]
const CURSOR_BITS: [u8; CURSOR_HEIGHT] = [
    0b1000_0000,
    0b1100_0000,
    0b1110_0000,
    0b1111_0000,
    0b1111_1000,
    0b1111_1100,
    0b1111_1110,
    0b1111_1111,
    0b1111_0000,
    0b1100_1000,
    0b1000_1100,
    0b0000_0110,
];

/// 按 `ratio/255` 混合两个颜色。
#[cfg(target_os = "none")]
fn blend(from: u32, to: u32, ratio: u32) -> u32 {
    let channel = |shift: u32| {
        let a = (from >> shift) & 0xFF;
        let b = (to >> shift) & 0xFF;
        ((a * (255 - ratio) + b * ratio) / 255) & 0xFF
    };
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

/// 返回桌面在指定行的颜色。
#[cfg(target_os = "none")]
fn desktop_color(height: usize, row: usize) -> u32 {
    let palette = theme();
    let ratio = (row * 255 / height.max(1)) as u32;
    blend(palette.desktop_top, palette.desktop_bottom, ratio)
}

/// 读取窗口表面中的一个像素。
#[cfg(target_os = "none")]
pub(crate) fn surface_pixel(window: &Window, x: usize, y: usize) -> u32 {
    // SAFETY: 表面是客户端转移过来的共享 Frame，长度已在创建时校验。
    unsafe {
        (window.virt as *const u32)
            .add(y * window.width + x)
            .read_volatile()
    }
}

/// 绘制鼠标箭头（带 1 像素阴影）。
#[cfg(target_os = "none")]
fn draw_cursor(screen: &Screen, x: usize, y: usize) {
    let palette = theme();
    for (row, bits) in CURSOR_BITS.iter().enumerate() {
        for column in 0..CURSOR_WIDTH {
            if bits & (0x80 >> column) != 0 {
                screen.put(x + column + 1, y + row + 1, palette.cursor_shadow);
            }
        }
    }
    for (row, bits) in CURSOR_BITS.iter().enumerate() {
        for column in 0..CURSOR_WIDTH {
            if bits & (0x80 >> column) != 0 {
                screen.put(x + column, y + row, palette.cursor);
            }
        }
    }
}

/// 在给定矩形内绘制一个窗口：标题栏、标题文字与表面。
#[cfg(target_os = "none")]
#[allow(clippy::too_many_arguments)]
fn paint_window(
    screen: &Screen,
    window: &Window,
    focus: u64,
    font: &Font<'static>,
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
) {
    let palette = theme();
    let window_x = window.x;
    let window_y = window.y;

    let title_left = window_x.max(left);
    let title_right = (window_x + window.width).min(right);
    let title_top = window_y.max(top);
    let title_bottom = (window_y + TITLE_HEIGHT).min(bottom);
    if title_left < title_right && title_top < title_bottom {
        let color = if window.id == focus {
            palette.title_focus
        } else {
            palette.title_color
        };
        for row in title_top..title_bottom {
            for column in title_left..title_right {
                screen.put(column, row, color);
            }
        }
        let mut pen_x = window_x as i32 + 4;
        for &byte in &window.title[..window.title_length] {
            if !(0x20..0x7F).contains(&byte) {
                pen_x += 8;
                continue;
            }
            if pen_x + 8 > (window_x + window.width) as i32 {
                break;
            }
            if let Some(glyph) = font.glyph(byte as char) {
                let origin_y = window_y as i32 + 4;
                glyph.for_each_pixel(|x, y| {
                    let pixel_x = pen_x + x;
                    let pixel_y = origin_y + y;
                    if pixel_x >= left as i32
                        && pixel_x < right as i32
                        && pixel_y >= top as i32
                        && pixel_y < bottom as i32
                        && pixel_y < (window_y + TITLE_HEIGHT) as i32
                    {
                        screen.put(pixel_x as usize, pixel_y as usize, palette.title_text);
                    }
                });
            }
            pen_x += 8;
        }
        let (close_x, close_y) = close_button_origin(window);
        for offset in 0..CLOSE_SIZE {
            let row = close_y + offset;
            if row < top || row >= bottom || row >= window_y + TITLE_HEIGHT {
                continue;
            }
            let left_pixel = close_x + offset;
            let right_pixel = close_x + (CLOSE_SIZE - 1 - offset);
            if left_pixel >= left && left_pixel < right {
                screen.put(left_pixel, row, palette.title_close);
            }
            if right_pixel >= left && right_pixel < right {
                screen.put(right_pixel, row, palette.title_close);
            }
        }
    }

    let body_left = window_x.max(left);
    let body_right = (window_x + window.width).min(right);
    let body_top = (window_y + TITLE_HEIGHT).max(top);
    let body_bottom = (window_y + TITLE_HEIGHT + window.height).min(bottom);
    for row in body_top..body_bottom {
        let source_row = row - (window_y + TITLE_HEIGHT);
        for column in body_left..body_right {
            screen.put(
                column,
                row,
                surface_pixel(window, column - window_x, source_row),
            );
        }
    }
}

/// 绘制任务栏及其窗口按钮。
#[cfg(target_os = "none")]
#[allow(clippy::too_many_arguments)]
fn paint_taskbar(
    screen: &Screen,
    windows: &[Option<Window>; MAX_WINDOWS],
    focus: u64,
    font: &Font<'static>,
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
) {
    let palette = theme();
    let bar_top = taskbar_top(screen.height);
    let top = top.max(bar_top);
    let bottom = bottom.min(screen.height);
    if left >= right || top >= bottom {
        return;
    }
    for row in top..bottom {
        for column in left..right {
            screen.put(column, row, palette.taskbar_bg);
        }
    }

    let button_top = taskbar_button_top(screen.height);
    if button_top >= bottom || button_top + TASKBAR_BUTTON_HEIGHT <= top {
        return;
    }
    for (index, window) in windows.iter().flatten().enumerate() {
        let button_x = taskbar_button_x(index);
        if button_x >= screen.width {
            break;
        }
        let button_width = TASKBAR_BUTTON_WIDTH.min(screen.width - button_x);
        let color = if window.id == focus {
            palette.taskbar_active
        } else {
            palette.taskbar_button
        };
        let fill_top = button_top.max(top);
        let fill_bottom = (button_top + TASKBAR_BUTTON_HEIGHT).min(bottom);
        for row in fill_top..fill_bottom {
            for column in button_x.max(left)..(button_x + button_width).min(right) {
                screen.put(column, row, color);
            }
        }

        let mut pen_x = button_x as i32 + 6;
        let text_end = (button_x + button_width) as i32 - 6;
        let title: &[u8] = if window.title_length == 0 {
            b"window"
        } else {
            &window.title[..window.title_length]
        };
        for &byte in title {
            if pen_x + 8 > text_end {
                break;
            }
            if (0x20..0x7F).contains(&byte) {
                if let Some(glyph) = font.glyph(byte as char) {
                    let origin_y = button_top as i32 + 2;
                    glyph.for_each_pixel(|x, y| {
                        let pixel_x = pen_x + x;
                        let pixel_y = origin_y + y;
                        if pixel_x >= left as i32
                            && pixel_x < right as i32
                            && pixel_y >= top as i32
                            && pixel_y < bottom as i32
                            && pixel_y >= bar_top as i32
                            && pixel_y < screen.height as i32
                        {
                            screen.put(pixel_x as usize, pixel_y as usize, palette.taskbar_text);
                        }
                    });
                }
            }
            pen_x += 8;
        }
    }
}

/// 重绘一个矩形区域：背景 + 自下而上的窗口。
#[cfg(target_os = "none")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn compose_region(
    screen: &Screen,
    windows: &[Option<Window>; MAX_WINDOWS],
    focus: u64,
    font: &Font<'static>,
    pointer: Option<(usize, usize)>,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
) {
    let left = x.min(screen.width);
    let top = y.min(screen.height);
    let right = x.saturating_add(width).min(screen.width);
    let bottom = y.saturating_add(height).min(screen.height);
    if left >= right || top >= bottom {
        return;
    }
    for row in top..bottom {
        let color = desktop_color(screen.height, row);
        for column in left..right {
            screen.put(column, row, color);
        }
    }
    for window in windows.iter().flatten() {
        paint_window(screen, window, focus, font, left, top, right, bottom);
    }
    paint_taskbar(screen, windows, focus, font, left, top, right, bottom);
    if let Some((cursor_x, cursor_y)) = pointer
        && cursor_x >= left
        && cursor_x < right
        && cursor_y >= top
        && cursor_y < bottom
    {
        draw_cursor(screen, cursor_x, cursor_y);
    }
}

/// 重绘整个桌面。
#[cfg(target_os = "none")]
pub(crate) fn draw_desktop(screen: &Screen) {
    for row in 0..screen.height {
        let color = desktop_color(screen.height, row);
        for column in 0..screen.width {
            screen.put(column, row, color);
        }
    }
}

/// 重绘窗口（含标题栏）。
#[cfg(target_os = "none")]
pub(crate) fn damage_window(
    screen: &Screen,
    windows: &[Option<Window>; MAX_WINDOWS],
    focus: u64,
    font: &Font<'static>,
    pointer: Option<(usize, usize)>,
    window: &Window,
) {
    compose_region(
        screen,
        windows,
        focus,
        font,
        pointer,
        window.x,
        window.y,
        window.width,
        window.height + TITLE_HEIGHT,
    );
}

/// 重绘任务栏。
#[cfg(target_os = "none")]
pub(crate) fn damage_taskbar(
    screen: &Screen,
    windows: &[Option<Window>; MAX_WINDOWS],
    focus: u64,
    font: &Font<'static>,
    pointer: Option<(usize, usize)>,
) {
    compose_region(
        screen,
        windows,
        focus,
        font,
        pointer,
        0,
        taskbar_top(screen.height),
        screen.width,
        TASKBAR_HEIGHT,
    );
}

/// 重绘指定窗口的标题栏。
#[cfg(target_os = "none")]
pub(crate) fn damage_title(
    screen: &Screen,
    windows: &[Option<Window>; MAX_WINDOWS],
    focus: u64,
    font: &Font<'static>,
    pointer: Option<(usize, usize)>,
    id: u64,
) {
    if id == 0 {
        return;
    }
    if let Some(window) = windows
        .iter()
        .flatten()
        .find(|window| window.id == id)
        .copied()
    {
        compose_region(
            screen,
            windows,
            focus,
            font,
            pointer,
            window.x,
            window.y,
            window.width,
            TITLE_HEIGHT,
        );
    }
}

/// 把窗口移动到新的左上角，并重绘旧、新两个区域。
#[cfg(target_os = "none")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn move_window(
    screen: &Screen,
    windows: &mut [Option<Window>; MAX_WINDOWS],
    focus: u64,
    font: &Font<'static>,
    old: Window,
    x: usize,
    y: usize,
) {
    let Some(window) = windows
        .iter_mut()
        .flatten()
        .find(|window| window.id == old.id)
    else {
        return;
    };
    window.x = x;
    window.y = y;
    compose_region(
        screen,
        windows,
        focus,
        font,
        None,
        old.x,
        old.y,
        old.width,
        old.height + TITLE_HEIGHT,
    );
    compose_region(
        screen,
        windows,
        focus,
        font,
        None,
        x,
        y,
        old.width,
        old.height + TITLE_HEIGHT,
    );
}
