//! Display 合成：桌面、窗口、光标与 damage 区域重绘。

use hk_font::Font;

use crate::theme::{scale, theme};
use crate::window::{
    MAX_WINDOWS, Screen, Window, close_button_origin, close_size, maximize_button_origin,
    minimize_button_origin, resize_size, taskbar_button_height, taskbar_button_top,
    taskbar_button_width, taskbar_button_x, taskbar_height, taskbar_top, title_height,
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

/// 窗口投影边距（随界面缩放）。
#[cfg(target_os = "none")]
fn shadow_margin() -> usize {
    4 * scale()
}

/// 窗口圆角半径（随界面缩放）。
#[cfg(target_os = "none")]
fn corner_radius() -> usize {
    6 * scale()
}

/// 整数平方根；半径很小，线性探测足够。
#[cfg(target_os = "none")]
fn sqrt_floor(value: usize) -> usize {
    let mut root = 0;
    while (root + 1) * (root + 1) <= value {
        root += 1;
    }
    root
}

/// 距离窗口边缘 `distance` 行/列处的圆角内缩量。
#[cfg(target_os = "none")]
fn corner_inset(distance: usize, radius: usize) -> usize {
    if distance >= radius || radius == 0 {
        return 0;
    }
    let radius = radius as u32;
    let dy = radius - distance as u32;
    (radius - sqrt_floor((radius * radius - dy * dy) as usize) as u32) as usize
}

/// 窗口在屏幕上是否需要圆角；最大化窗口铺满宽度，不剪角。
#[cfg(target_os = "none")]
fn window_radius(screen: &Screen, window: &Window) -> usize {
    if window.width >= screen.width {
        0
    } else {
        corner_radius()
    }
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
    let source_x = x
        .checked_mul(window.surface_width)
        .and_then(|value| value.checked_div(window.width))
        .map_or(0, |value| value.min(window.surface_width.saturating_sub(1)));
    let source_y = y
        .checked_mul(window.surface_height)
        .and_then(|value| value.checked_div(window.height))
        .map_or(0, |value| {
            value.min(window.surface_height.saturating_sub(1))
        });
    // SAFETY: 表面是客户端转移过来的共享 Frame，长度已在创建时校验。
    unsafe {
        (window.virt as *const u32)
            .add(source_y * window.surface_width + source_x)
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

/// 绘制窗口外沿的柔和投影。
#[cfg(target_os = "none")]
fn paint_shadow(
    screen: &Screen,
    window: &Window,
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
) {
    if window.minimized || window.width >= screen.width {
        return;
    }
    let margin = shadow_margin();
    if margin == 0 {
        return;
    }
    let palette = theme();
    let window_right = window.x.saturating_add(window.width);
    let window_bottom = window
        .y
        .saturating_add(title_height())
        .saturating_add(window.height);
    let shadow_left = window.x.saturating_sub(margin).max(left);
    let shadow_top = window.y.saturating_sub(margin).max(top);
    let shadow_right = window_right.saturating_add(margin).min(right);
    let shadow_bottom = window_bottom.saturating_add(margin).min(bottom);
    if shadow_left >= shadow_right || shadow_top >= shadow_bottom {
        return;
    }

    for row in shadow_top..shadow_bottom {
        let dy = if row < window.y {
            window.y - row
        } else if row >= window_bottom {
            row - window_bottom + 1
        } else {
            0
        };
        if dy == 0 && row >= window.y && row < window_bottom {
            // 与窗口同高的行只扫描左右两条边带，不遍历窗口内部。
            for column in shadow_left..window.x.min(shadow_right) {
                let dx = window.x - column;
                let distance = dx.max(dy);
                let ratio = ((margin + 1 - distance) * 140 / (margin + 1)) as u32;
                screen.blend(column, row, palette.shadow, ratio);
            }
            for column in window_right.max(shadow_left)..shadow_right {
                let dx = column - window_right + 1;
                let distance = dx.max(dy);
                let ratio = ((margin + 1 - distance) * 140 / (margin + 1)) as u32;
                screen.blend(column, row, palette.shadow, ratio);
            }
        } else {
            for column in shadow_left..shadow_right {
                let dx = if column < window.x {
                    window.x - column
                } else if column >= window_right {
                    column - window_right + 1
                } else {
                    0
                };
                let distance = dx.max(dy);
                if distance == 0 || distance > margin {
                    continue;
                }
                let ratio = ((margin + 1 - distance) * 140 / (margin + 1)) as u32;
                screen.blend(column, row, palette.shadow, ratio);
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
    font: &Font,
    left: usize,
    top: usize,
    right: usize,
    bottom: usize,
) {
    if window.minimized {
        return;
    }
    paint_shadow(screen, window, left, top, right, bottom);
    let palette = theme();
    let window_x = window.x;
    let window_y = window.y;
    let radius = window_radius(screen, window);

    let title_left = window_x.max(left);
    let title_right = (window_x + window.width).min(right);
    let title_top = window_y.max(top);
    let title_bottom = (window_y + title_height()).min(bottom);
    if title_left < title_right && title_top < title_bottom {
        let color = if window.id == focus {
            palette.title_focus
        } else {
            palette.title_color
        };
        for row in title_top..title_bottom {
            let inset = corner_inset(row.saturating_sub(window_y), radius);
            let row_left = window_x.saturating_add(inset).max(title_left);
            let row_right = (window_x + window.width)
                .saturating_sub(inset)
                .min(title_right);
            if row_left < row_right {
                screen.fill_row(row_left, row, row_right - row_left, color);
            }
        }
        let font_scale = scale() as i32;
        let origin_y = window_y as i32 + 4 * font_scale;
        let text_end = (window_x + window.width) as i32;
        let mut pen_x = window_x as i32 + 4 * font_scale;
        for &byte in &window.title[..window.title_length] {
            if !(0x20..0x7F).contains(&byte) {
                pen_x += 8 * font_scale;
                continue;
            }
            let Some(glyph) = font.glyph(byte as char) else {
                pen_x += 8 * font_scale;
                continue;
            };
            let advance = glyph.metric().xadvance;
            if pen_x + advance > text_end {
                break;
            }
            glyph.for_each_pixel(|x, y| {
                let pixel_x = pen_x + x;
                let pixel_y = origin_y + y;
                if pixel_x >= left as i32
                    && pixel_x < right as i32
                    && pixel_y >= top as i32
                    && pixel_y < bottom as i32
                    && pixel_y < (window_y + title_height()) as i32
                {
                    screen.put(pixel_x as usize, pixel_y as usize, palette.title_text);
                }
            });
            pen_x += advance;
        }
        let (close_x, close_y) = close_button_origin(window);
        for offset in 0..close_size() {
            let row = close_y + offset;
            if row < top || row >= bottom || row >= window_y + title_height() {
                continue;
            }
            let left_pixel = close_x + offset;
            let right_pixel = close_x + (close_size() - 1 - offset);
            if left_pixel >= left && left_pixel < right {
                screen.put(left_pixel, row, palette.title_close);
            }
            if right_pixel >= left && right_pixel < right {
                screen.put(right_pixel, row, palette.title_close);
            }
        }

        let (maximize_x, maximize_y) = maximize_button_origin(window);
        let (minimize_x, minimize_y) = minimize_button_origin(window);
        let inside = |x: usize, y: usize| {
            x >= left && x < right && y >= top && y < bottom && y < window_y + title_height()
        };
        let button_bottom = maximize_y + close_size() - 1;
        for offset in 0..close_size() {
            let x = maximize_x + offset;
            if inside(x, maximize_y) {
                screen.put(x, maximize_y, palette.title_text);
            }
            if inside(x, button_bottom) {
                screen.put(x, button_bottom, palette.title_text);
            }
        }
        for offset in 0..close_size() {
            let y = maximize_y + offset;
            if inside(maximize_x, y) {
                screen.put(maximize_x, y, palette.title_text);
            }
            if inside(maximize_x + close_size() - 1, y) {
                screen.put(maximize_x + close_size() - 1, y, palette.title_text);
            }
        }
        let minimize_line = minimize_y + close_size() / 2;
        for offset in 2..close_size() - 2 {
            let x = minimize_x + offset;
            if inside(x, minimize_line) {
                screen.put(x, minimize_line, palette.title_text);
            }
        }
    }

    let body_left = window_x.max(left);
    let body_right = (window_x + window.width).min(right);
    let body_top = (window_y + title_height()).max(top);
    let body_bottom = (window_y + title_height() + window.height).min(bottom);
    if body_left < body_right
        && body_top < body_bottom
        && window.width == window.surface_width
        && window.height == window.surface_height
    {
        let window_bottom = window_y + title_height() + window.height;
        for row in body_top..body_bottom {
            let distance = window_bottom.saturating_sub(1).saturating_sub(row);
            let inset = corner_inset(distance, radius);
            let row_left = window_x.saturating_add(inset).max(body_left);
            let row_right = (window_x + window.width)
                .saturating_sub(inset)
                .min(body_right);
            if row_left >= row_right {
                continue;
            }
            let count = row_right - row_left;
            let source_x = row_left - window_x;
            let source_row = row - (window_y + title_height());
            // SAFETY: 表面按 surface_width×surface_height 映射，坐标已裁剪到窗口内。
            let source = unsafe {
                (window.virt as *const u32).add(source_row * window.surface_width + source_x)
            };
            screen.copy_row(row_left, row, count, source);
        }
    } else {
        let width = window.width.max(1);
        let height = window.height.max(1);
        let x_step = (window.surface_width as u64 * 65536 / width as u64) as usize;
        let window_bottom = window_y + title_height() + window.height;
        for row in body_top..body_bottom {
            let distance = window_bottom.saturating_sub(1).saturating_sub(row);
            let inset = corner_inset(distance, radius);
            let row_left = window_x.saturating_add(inset).max(body_left);
            let row_right = (window_x + window.width)
                .saturating_sub(inset)
                .min(body_right);
            if row_left >= row_right {
                continue;
            }
            let source_row = row - (window_y + title_height());
            let source_y =
                (source_row as u64 * window.surface_height as u64 / height as u64) as usize;
            // SAFETY: 采样坐标已按表面尺寸缩放。
            let source =
                unsafe { (window.virt as *const u32).add(source_y * window.surface_width) };
            let start_x = row_left - window_x;
            let mut column_source =
                ((start_x as u64 * window.surface_width as u64 * 65536) / width as u64) as usize;
            for column in row_left..row_right {
                let sample = (column_source >> 16).min(window.surface_width - 1);
                // SAFETY: sample < surface_width。
                let color = unsafe { source.add(sample).read() };
                screen.put(column, row, color);
                column_source += x_step;
            }
        }
    }

    if window.id == focus {
        let right = window_x.saturating_add(window.width);
        let bottom = window_y
            .saturating_add(title_height())
            .saturating_add(window.height);
        for row in 0..resize_size() {
            let y = bottom.saturating_sub(1 + row);
            if y < top || y >= bottom || y < window_y + title_height() {
                continue;
            }
            for column in 0..resize_size() - row {
                let x = right.saturating_sub(1 + column);
                if x >= left && x < right && x >= window_x {
                    screen.put(x, y, palette.resize_handle);
                }
            }
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
    font: &Font,
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
        screen.fill_row(left, row, right - left, palette.taskbar_bg);
    }

    let button_top = taskbar_button_top(screen.height);
    if button_top >= bottom || button_top + taskbar_button_height() <= top {
        return;
    }
    for (index, window) in windows.iter().flatten().enumerate() {
        let button_x = taskbar_button_x(index);
        if button_x >= screen.width {
            break;
        }
        let button_width = taskbar_button_width().min(screen.width - button_x);
        let color = if window.id == focus {
            palette.taskbar_active
        } else {
            palette.taskbar_button
        };
        let fill_top = button_top.max(top);
        let fill_bottom = (button_top + taskbar_button_height()).min(bottom);
        let fill_left = button_x.max(left);
        let fill_right = (button_x + button_width).min(right);
        for row in fill_top..fill_bottom {
            screen.fill_row(fill_left, row, fill_right.saturating_sub(fill_left), color);
        }

        let font_scale = scale() as i32;
        let mut pen_x = button_x as i32 + 6 * font_scale;
        let text_end = (button_x + button_width) as i32 - 6 * font_scale;
        let title: &[u8] = if window.title_length == 0 {
            b"window"
        } else {
            &window.title[..window.title_length]
        };
        for &byte in title {
            if !(0x20..0x7F).contains(&byte) {
                pen_x += 8 * font_scale;
                continue;
            }
            let Some(glyph) = font.glyph(byte as char) else {
                pen_x += 8 * font_scale;
                continue;
            };
            let advance = glyph.metric().xadvance;
            if pen_x + advance > text_end {
                break;
            }
            let origin_y = button_top as i32 + 2 * font_scale;
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
            pen_x += advance;
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
    font: &Font,
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
    screen.mark_damage(x, y, width, height);
    for row in top..bottom {
        let color = desktop_color(screen.height, row);
        screen.fill_row(left, row, right - left, color);
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

/// 重绘窗口及其投影覆盖的区域。
#[cfg(target_os = "none")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn compose_window_region(
    screen: &Screen,
    windows: &[Option<Window>; MAX_WINDOWS],
    focus: u64,
    font: &Font,
    pointer: Option<(usize, usize)>,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
) {
    let margin = shadow_margin();
    compose_region(
        screen,
        windows,
        focus,
        font,
        pointer,
        x.saturating_sub(margin),
        y.saturating_sub(margin),
        width.saturating_add(margin * 2),
        height.saturating_add(margin * 2),
    );
}

/// 重绘整个桌面。
#[cfg(target_os = "none")]
pub(crate) fn draw_desktop(screen: &Screen) {
    screen.mark_damage(0, 0, screen.width, screen.height);
    for row in 0..screen.height {
        let color = desktop_color(screen.height, row);
        screen.fill_row(0, row, screen.width, color);
    }
}

/// 重绘窗口（含标题栏）。
#[cfg(target_os = "none")]
pub(crate) fn damage_window(
    screen: &Screen,
    windows: &[Option<Window>; MAX_WINDOWS],
    focus: u64,
    font: &Font,
    pointer: Option<(usize, usize)>,
    window: &Window,
) {
    compose_window_region(
        screen,
        windows,
        focus,
        font,
        pointer,
        window.x,
        window.y,
        window.width,
        window.height + title_height(),
    );
}

/// 重绘任务栏。
#[cfg(target_os = "none")]
pub(crate) fn damage_taskbar(
    screen: &Screen,
    windows: &[Option<Window>; MAX_WINDOWS],
    focus: u64,
    font: &Font,
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
        taskbar_height(),
    );
}

/// 重绘指定窗口的标题栏。
#[cfg(target_os = "none")]
pub(crate) fn damage_title(
    screen: &Screen,
    windows: &[Option<Window>; MAX_WINDOWS],
    focus: u64,
    font: &Font,
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
        compose_window_region(
            screen,
            windows,
            focus,
            font,
            pointer,
            window.x,
            window.y,
            window.width,
            title_height(),
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
    font: &Font,
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
    window.restore = None;
    compose_window_region(
        screen,
        windows,
        focus,
        font,
        None,
        old.x,
        old.y,
        old.width,
        old.height + title_height(),
    );
    compose_window_region(
        screen,
        windows,
        focus,
        font,
        None,
        x,
        y,
        old.width,
        old.height + title_height(),
    );
}

/// 把窗口缩放到新尺寸，并重绘旧、新两个区域。
#[cfg(target_os = "none")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn resize_window(
    screen: &Screen,
    windows: &mut [Option<Window>; MAX_WINDOWS],
    focus: u64,
    font: &Font,
    old: Window,
    width: usize,
    height: usize,
) {
    let Some(window) = windows
        .iter_mut()
        .flatten()
        .find(|window| window.id == old.id)
    else {
        return;
    };
    window.width = width;
    window.height = height;
    window.restore = None;
    compose_window_region(
        screen,
        windows,
        focus,
        font,
        None,
        old.x,
        old.y,
        old.width,
        old.height + title_height(),
    );
    compose_window_region(
        screen,
        windows,
        focus,
        font,
        None,
        old.x,
        old.y,
        width,
        height + title_height(),
    );
}
