//! 窗口操作：最小化、最大化、关闭、焦点切换与按键路由。

use hk_font::Font;
use hk_proto::display::{
    DISPLAY_CLOSE, DISPLAY_KEY, DISPLAY_RESIZE, DISPLAY_WHEEL, pack_key, pack_wheel,
    pack_window_size,
};
use hk_server_api::{close_handle, format::Line, region_unmap, send};

use crate::compositor::{compose_window_region, damage_taskbar, damage_title, damage_window};
use crate::pointer::Cursor;
use crate::set_scanout;
use crate::window::{MAX_WINDOWS, Screen, Window, raise, taskbar_height, title_height};

/// 最小化窗口并把焦点交给剩余最上层窗口。
#[allow(clippy::too_many_arguments)]
pub(crate) fn minimize_window(
    screen: &Screen,
    windows: &mut [Option<Window>; MAX_WINDOWS],
    focus: &mut u64,
    font: &Font,
    cursor: Option<Cursor>,
    id: u64,
) {
    let Some(window) = windows.iter_mut().flatten().find(|window| window.id == id) else {
        return;
    };
    window.minimized = true;
    set_scanout(window.surface.handle, window.surface.id, false);
    let old = *window;
    let previous = *focus;
    if previous == id {
        *focus = windows
            .iter()
            .flatten()
            .rfind(|window| !window.minimized)
            .map(|window| window.id)
            .unwrap_or(0);
    }
    compose_window_region(
        screen,
        windows,
        *focus,
        font,
        cursor,
        old.x,
        old.y,
        old.width,
        old.height + title_height(),
    );
    damage_title(screen, windows, *focus, font, cursor, previous);
    damage_taskbar(screen, windows, *focus, font, cursor);
    Line::new()
        .text(b"hk-server-display: minimize ")
        .number(id)
        .text(b"\n")
        .finish();
}

/// 最大化或还原窗口。
#[allow(clippy::too_many_arguments)]
pub(crate) fn toggle_maximize(
    screen: &Screen,
    windows: &mut [Option<Window>; MAX_WINDOWS],
    focus: u64,
    font: &Font,
    cursor: Option<Cursor>,
    id: u64,
) {
    let desktop_height = screen.height.saturating_sub(taskbar_height());
    let Some(window) = windows.iter_mut().flatten().find(|window| window.id == id) else {
        return;
    };
    let old = *window;
    let maximized = if let Some((x, y, width, height)) = window.restore.take() {
        window.x = x;
        window.y = y;
        window.width = width;
        window.height = height;
        false
    } else {
        window.restore = Some((old.x, old.y, old.width, old.height));
        window.x = 0;
        window.y = 0;
        window.width = screen.width;
        window.height = desktop_height.saturating_sub(title_height());
        true
    };
    let new = *window;
    compose_window_region(
        screen,
        windows,
        focus,
        font,
        cursor,
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
        cursor,
        new.x,
        new.y,
        new.width,
        new.height + title_height(),
    );
    damage_title(screen, windows, focus, font, cursor, id);
    damage_taskbar(screen, windows, focus, font, cursor);
    Line::new()
        .text(if maximized {
            b"hk-server-display: maximize ".as_slice()
        } else {
            b"hk-server-display: restore ".as_slice()
        })
        .number(id)
        .text(b" ")
        .number(new.width as u64)
        .text(b"x")
        .number(new.height as u64)
        .text(b"\n")
        .finish();
    let _ = send(
        new.surface.client,
        DISPLAY_RESIZE,
        pack_window_size(id, new.width as u32, new.height as u32),
    );
}

/// 关闭窗口：回收共享表面、通知客户端并更新焦点与任务栏。
#[allow(clippy::too_many_arguments)]
pub(crate) fn close_window(
    screen: &Screen,
    windows: &mut [Option<Window>; MAX_WINDOWS],
    focus: &mut u64,
    font: &Font,
    cursor: Option<Cursor>,
    id: u64,
) {
    let Some(slot) = windows
        .iter()
        .position(|window| window.as_ref().is_some_and(|window| window.id == id))
    else {
        return;
    };
    let Some(window) = windows[slot] else {
        return;
    };
    windows[slot] = None;
    set_scanout(window.surface.handle, window.surface.id, false);
    let _ = region_unmap(window.surface.region);
    let _ = close_handle(window.surface.handle);
    let _ = send(window.surface.client, DISPLAY_CLOSE, id);

    let previous = *focus;
    if previous == id {
        *focus = windows
            .iter()
            .flatten()
            .last()
            .map(|window| window.id)
            .unwrap_or(0);
    }
    compose_window_region(
        screen,
        windows,
        *focus,
        font,
        cursor,
        window.x,
        window.y,
        window.width,
        window.height + title_height(),
    );
    damage_title(screen, windows, *focus, font, cursor, previous);
    damage_taskbar(screen, windows, *focus, font, cursor);
    Line::new()
        .text(b"hk-server-display: close ")
        .number(id)
        .text(b"\n")
        .finish();
}

/// 把按键转发给焦点窗口的客户端；没有焦点时丢弃。
pub(crate) fn route_key(windows: &[Option<Window>; MAX_WINDOWS], focus: u64, byte: u8) {
    if let Some(window) = windows.iter().flatten().find(|window| window.id == focus) {
        let _ = send(
            window.surface.client,
            DISPLAY_KEY,
            pack_key(window.id, byte),
        );
    }
}

/// 把滚轮增量转发给焦点窗口的客户端；没有焦点时丢弃。
pub(crate) fn route_wheel(windows: &[Option<Window>; MAX_WINDOWS], focus: u64, delta: i32) {
    if let Some(window) = windows.iter().flatten().find(|window| window.id == focus) {
        let _ = send(
            window.surface.client,
            DISPLAY_WHEEL,
            pack_wheel(window.id, delta),
        );
    }
}

/// Tab：把焦点移到下一个窗口并提升它。
pub(crate) fn cycle_focus(
    screen: &Screen,
    windows: &mut [Option<Window>; MAX_WINDOWS],
    focus: &mut u64,
    font: &Font,
    cursor: Option<Cursor>,
) {
    let count = windows
        .iter()
        .flatten()
        .filter(|window| !window.minimized)
        .count();
    if count == 0 {
        return;
    }
    let position = windows
        .iter()
        .flatten()
        .filter(|window| !window.minimized)
        .position(|window| window.id == *focus)
        .unwrap_or(count - 1);
    let next = (position + 1) % count;
    let Some(id) = windows
        .iter()
        .flatten()
        .filter(|window| !window.minimized)
        .nth(next)
        .map(|window| window.id)
    else {
        return;
    };
    let previous = *focus;
    raise(windows, id);
    *focus = id;
    if let Some(window) = windows
        .iter()
        .flatten()
        .find(|window| window.id == id)
        .copied()
    {
        damage_window(screen, windows, *focus, font, cursor, &window);
    }
    damage_title(screen, windows, *focus, font, cursor, previous);
    damage_taskbar(screen, windows, *focus, font, cursor);
    Line::new()
        .text(b"hk-server-display: focus ")
        .number(id)
        .text(b"\n")
        .finish();
}
