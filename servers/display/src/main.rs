//! display：帧缓冲合成器与窗口服务。
//!
//! 只有本服务持有帧缓冲授权。客户端把共享 Frame 作为窗口表面，经句柄转移
//! 交给本服务；合成按区域重绘，避免每次提交都重画整屏。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(any(target_os = "none", test))]
mod pointer;
#[cfg(any(target_os = "none", test))]
mod resize;
#[cfg(any(target_os = "none", test))]
mod seat;
#[cfg(any(target_os = "none", test))]
mod surface;

#[cfg(target_os = "none")]
use hk_font::Font;
#[cfg(target_os = "none")]
use hk_proto::display::{
    DISPLAY_CLICK, DISPLAY_CLICK_AT, DISPLAY_RESIZE, DISPLAY_RESTART, DISPLAY_SET_TITLE,
    DISPLAY_SHUTDOWN, SEAT_CLIPBOARD, SEAT_FOCUS, SEAT_GET_CLIPBOARD, SEAT_SET_CLIPBOARD,
    SURFACE_COMMIT, SURFACE_CREATE, SURFACE_CREATED, SURFACE_DAMAGE, SURFACE_DESTROY,
    SURFACE_FRAME_DONE, SURFACE_SET_CURSOR, SURFACE_SET_INPUT_REGION, TITLE_MAX, WINDOW_CREATE,
    WINDOW_CREATED, WINDOW_RESIZE, pack_click_at, pack_window_size, unpack_size,
    unpack_surface_rect, unpack_window_size,
};
#[cfg(target_os = "none")]
use hk_proto::input::{
    INPUT_BUTTON, INPUT_KEY, INPUT_POINTER, INPUT_WHEEL, unpack_pointer, unpack_wheel,
};
#[cfg(target_os = "none")]
use hk_proto::pointer::{Shape, unpack_surface_cursor};
#[cfg(target_os = "none")]
use hk_server_api::framebuffer;
#[cfg(target_os = "none")]
use hk_server_api::{
    CapabilityKind, MemoryClient, capability_phys, capability_scanout, close_handle, console_read,
    exit, map_attach, recv_data_handle_try, region_unmap, send, send_data, tsc, tsc_frequency,
    write, write_u64, yield_now,
};
#[cfg(target_os = "none")]
#[cfg(target_os = "none")]
mod actions;
#[cfg(target_os = "none")]
use actions::{
    close_window, cycle_focus, minimize_window, route_key, route_wheel, toggle_maximize,
};

#[cfg(target_os = "none")]
mod compositor;
#[cfg(target_os = "none")]
use compositor::{
    compose_region, damage_taskbar, damage_title, damage_window, draw_desktop, move_window,
    resize_window, surface_pixel,
};
#[cfg(target_os = "none")]
use pointer::{Cursor, Hover};

#[cfg(target_os = "none")]
use resize::Rect;
#[cfg(target_os = "none")]
use seat::Seat;
#[cfg(target_os = "none")]
use surface::{MAX_SURFACES, Surface, clip};

#[cfg(target_os = "none")]
mod theme;
#[cfg(target_os = "none")]
mod window;
#[cfg(target_os = "none")]
use window::{
    Drag, MAX_WINDOWS, MIN_WINDOW_HEIGHT, MIN_WINDOW_WIDTH, Resize, Screen, Window, close_at,
    maximize_at, minimize_at, raise, resize_at, taskbar_at, taskbar_height, taskbar_top,
    title_height, window_at,
};

/// Tab 键字节：切换焦点。
#[cfg(target_os = "none")]
const TAB: u8 = b'\t';

/// 全屏合成并提交，返回耗时 cycles。
#[cfg(target_os = "none")]
fn measure_full_redraw(
    screen: &Screen,
    windows: &[Option<Window>; MAX_WINDOWS],
    focus: u64,
    font: &Font,
    cursor: Option<Cursor>,
) -> u64 {
    let start = tsc();
    compose_region(
        screen,
        windows,
        focus,
        font,
        cursor,
        0,
        0,
        screen.width,
        screen.height,
    );
    screen.present();
    tsc().saturating_sub(start)
}

/// 输出全屏重绘耗时。
#[cfg(target_os = "none")]
fn report_full_redraw(label: &[u8], cycles: u64) {
    let frequency = tsc_frequency();
    write(b"hk-server-display: ");
    write(label);
    let micros = cycles.saturating_mul(1_000_000).checked_div(frequency);
    write_u64(micros.unwrap_or(cycles));
    write(if micros.is_some() {
        b" us\n".as_slice()
    } else {
        b" cycles\n".as_slice()
    });
}

/// 创建表面：校验共享 Frame、映射到表面槽并登记。
#[cfg(target_os = "none")]
fn create_surface(
    windows: &[Option<Window>; MAX_WINDOWS],
    surfaces: &mut [Option<Surface>; MAX_SURFACES],
    next_id: &mut u64,
    client: u64,
    width: u32,
    height: u32,
    handle: u64,
) -> Option<u64> {
    let width = width as usize;
    let height = height as usize;
    if handle == 0 || width == 0 || height == 0 {
        if handle != 0 {
            let _ = close_handle(handle);
        }
        write(b"hk-server-display: create invalid\n");
        return None;
    }
    let free = |slot: &usize| {
        surfaces[*slot].is_none() && !windows.iter().flatten().any(|window| window.slot == *slot)
    };
    let Some(slot) = (0..MAX_SURFACES).find(free) else {
        let _ = close_handle(handle);
        write(b"hk-server-display: create full\n");
        return None;
    };
    let bytes = width as u64 * height as u64 * 4;
    let Some((_, frame_length)) = capability_phys(handle) else {
        let _ = close_handle(handle);
        write(b"hk-server-display: create no-frame\n");
        return None;
    };
    if frame_length < bytes {
        let _ = close_handle(handle);
        write(b"hk-server-display: create short-frame\n");
        return None;
    }
    let id = *next_id;
    let Some((region, virt)) = map_attach(handle) else {
        let _ = close_handle(handle);
        write(b"hk-server-display: create map-failed\n");
        return None;
    };
    if capability_phys(handle).is_none_or(|(_, length)| length < bytes) {
        let _ = region_unmap(region);
        let _ = close_handle(handle);
        write(b"hk-server-display: create map-failed\n");
        return None;
    }
    set_scanout(handle, id, true);
    surfaces[slot] = Some(Surface::new(
        id, client, slot, width, height, virt, region, handle,
    ));
    *next_id += 1;
    Some(id)
}

/// 设置表面 scanout 驻留并记录状态切换。
#[cfg(target_os = "none")]
fn set_scanout(handle: u64, surface_id: u64, enabled: bool) {
    let _ = capability_scanout(handle, enabled);
    write(b"hk-server-display: scanout ");
    write_u64(surface_id);
    if enabled {
        write(b" on\n");
    } else {
        write(b" off\n");
    }
}

/// 丢弃未绑定表面的缓冲。
#[cfg(target_os = "none")]
fn discard_surface(surfaces: &mut [Option<Surface>; MAX_SURFACES], surface_id: u64) {
    if let Some(slot) = surfaces.iter().position(|entry| {
        entry
            .as_ref()
            .is_some_and(|surface| surface.id == surface_id)
    }) && let Some(surface) = surfaces[slot].take()
    {
        set_scanout(surface.handle, surface.id, false);
        let _ = region_unmap(surface.region);
        let _ = close_handle(surface.handle);
    }
}

/// 通知客户端窗口内容尺寸；提交帧后若仍不一致会再次补发。
#[cfg(target_os = "none")]
fn notify_resize(windows: &[Option<Window>; MAX_WINDOWS], id: u64) {
    if let Some(window) = windows.iter().flatten().find(|window| window.id == id) {
        let _ = send(
            window.surface.client,
            DISPLAY_RESIZE,
            pack_window_size(id, window.width as u32, window.height as u32),
        );
    }
}

/// 用客户端转移的新表面替换窗口缓冲，尺寸以客户端声明为准。
#[cfg(target_os = "none")]
#[allow(clippy::too_many_arguments)]
fn replace_window_surface(
    screen: &Screen,
    windows: &mut [Option<Window>; MAX_WINDOWS],
    focus: u64,
    font: &Font,
    cursor: Option<Cursor>,
    id: u64,
    width: usize,
    height: usize,
    handle: u64,
    sender: u64,
) -> bool {
    let Some(index) = windows
        .iter()
        .position(|window| window.as_ref().is_some_and(|window| window.id == id))
    else {
        if handle != 0 && handle != u64::MAX {
            let _ = close_handle(handle);
        }
        return false;
    };
    if handle == 0 || handle == u64::MAX || width == 0 || height == 0 {
        if handle != 0 && handle != u64::MAX {
            let _ = close_handle(handle);
        }
        return false;
    }
    let bytes = width as u64 * height as u64 * 4;
    let Some((_, frame_length)) = capability_phys(handle) else {
        let _ = close_handle(handle);
        write(b"hk-server-display: resize no-frame\n");
        return false;
    };
    if frame_length < bytes {
        let _ = close_handle(handle);
        write(b"hk-server-display: resize short-frame\n");
        return false;
    }
    let Some(window) = windows[index] else {
        let _ = close_handle(handle);
        return false;
    };
    if window.surface.client != sender {
        let _ = close_handle(handle);
        write(b"hk-server-display: resize owner mismatch\n");
        return false;
    }
    let (surface_id, client, slot) = (
        window.surface.id,
        window.surface.client,
        window.surface.slot,
    );
    let Some((region, virt)) = map_attach(handle) else {
        let _ = close_handle(handle);
        write(b"hk-server-display: resize map-failed\n");
        return false;
    };
    if capability_phys(handle).is_none_or(|(_, length)| length < bytes) {
        let _ = region_unmap(region);
        let _ = close_handle(handle);
        write(b"hk-server-display: resize map-failed\n");
        return false;
    }
    set_scanout(handle, surface_id, true);
    let old = window.surface;
    let mut replacement = Surface::new(
        surface_id, client, slot, width, height, virt, region, handle,
    );
    replacement.cursor = old.cursor;
    if let Some(window) = windows[index].as_mut() {
        window.surface = replacement;
    }
    set_scanout(old.handle, old.id, false);
    let _ = region_unmap(old.region);
    let _ = close_handle(old.handle);
    if let Some(window) = windows[index] {
        damage_window(screen, windows, focus, font, cursor, &window);
    }
    write(b"hk-server-display: surface resize ");
    write_u64(id);
    write(b" ");
    write_u64(width as u64);
    write(b"x");
    write_u64(height as u64);
    write(b"\n");
    true
}

/// 把表面绑定到窗口：分配窗口槽、放置几何并提升焦点。
#[cfg(target_os = "none")]
#[allow(clippy::too_many_arguments)]
fn attach_window(
    screen: &Screen,
    windows: &mut [Option<Window>; MAX_WINDOWS],
    surfaces: &mut [Option<Surface>; MAX_SURFACES],
    surface_id: u64,
    next_id: &mut u64,
    focus: &mut u64,
    font: &Font,
    cursor: Option<Cursor>,
) -> Option<u64> {
    let slot = surfaces.iter().position(|entry| {
        entry
            .as_ref()
            .is_some_and(|surface| surface.id == surface_id)
    })?;
    let surface = surfaces[slot].take()?;
    let Some(window_slot) = windows.iter().position(Option::is_none) else {
        surfaces[slot] = Some(surface);
        write(b"hk-server-display: create full\n");
        return None;
    };
    let id = *next_id;
    let width = surface.width;
    let height = surface.height;
    let offset = (id as usize - 1) * 16;
    let x = screen
        .width
        .saturating_sub(width)
        .saturating_div(2)
        .saturating_add(offset)
        .min(screen.width.saturating_sub(width));
    let desktop_height = screen.height.saturating_sub(taskbar_height());
    let y = desktop_height
        .saturating_sub(height + title_height())
        .saturating_div(2)
        .saturating_add(offset)
        .min(desktop_height.saturating_sub(height + title_height()));
    let previous = *focus;
    windows[window_slot] = Some(Window {
        id,
        slot: surface.slot,
        surface,
        x,
        y,
        width,
        height,
        title: [0; TITLE_MAX],
        title_length: 0,
        restore: None,
        minimized: false,
    });
    *next_id += 1;
    *focus = id;
    raise(windows, id);
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
    Some(id)
}

/// 在未绑定表面或已绑定窗口上记录 damage。
#[cfg(target_os = "none")]
fn mark_surface_damage(
    windows: &mut [Option<Window>; MAX_WINDOWS],
    surfaces: &mut [Option<Surface>; MAX_SURFACES],
    surface_id: u64,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
) -> bool {
    if let Some(window) = windows
        .iter_mut()
        .flatten()
        .find(|window| window.surface.id == surface_id)
    {
        window.surface.mark_damage(x, y, width, height);
        return true;
    }
    if let Some(surface) = surfaces
        .iter_mut()
        .flatten()
        .find(|surface| surface.id == surface_id)
    {
        surface.mark_damage(x, y, width, height);
        return true;
    }
    false
}

/// 设置未绑定表面或已绑定窗口的输入区域。
#[cfg(target_os = "none")]
fn set_surface_input_region(
    windows: &mut [Option<Window>; MAX_WINDOWS],
    surfaces: &mut [Option<Surface>; MAX_SURFACES],
    surface_id: u64,
    region: crate::resize::Rect,
) -> bool {
    if let Some(window) = windows
        .iter_mut()
        .flatten()
        .find(|window| window.surface.id == surface_id)
    {
        window.surface.set_input_region(region);
        return true;
    }
    if let Some(surface) = surfaces
        .iter_mut()
        .flatten()
        .find(|surface| surface.id == surface_id)
    {
        surface.set_input_region(region);
        return true;
    }
    false
}

/// 应用悬停目标：更新光标形状与按钮高亮。
#[cfg(target_os = "none")]
fn apply_hover(
    screen: &Screen,
    windows: &[Option<Window>; MAX_WINDOWS],
    focus: u64,
    font: &Font,
    seat: &mut Seat,
    hover: Hover,
    shape: Shape,
) {
    if seat.pointer.set_shape(shape) {
        write(b"hk-server-display: cursor ");
        write(pointer::label(shape).as_bytes());
        write(b"\n");
    }
    let previous = screen.hover();
    if previous == Some(hover) {
        return;
    }
    screen.set_hover(Some(hover));
    for target in [previous, Some(hover)] {
        if let Some((x, y, width, height)) = pointer::hover_rect(windows, screen, target) {
            compose_region(screen, windows, focus, font, None, x, y, width, height);
        }
    }
}

/// 擦除旧光标：重绘它覆盖的矩形但不画光标。
#[cfg(target_os = "none")]
fn erase_cursor(
    screen: &Screen,
    windows: &[Option<Window>; MAX_WINDOWS],
    focus: u64,
    font: &Font,
    cursor: Option<Cursor>,
) {
    if let Some(cursor) = cursor {
        let (x, y, width, height) =
            pointer::cursor_rect(cursor, screen.width, screen.height, theme::scale());
        compose_region(screen, windows, focus, font, None, x, y, width, height);
    }
}

/// 在当前指针位置重绘光标。
#[cfg(target_os = "none")]
fn redraw_cursor(
    screen: &Screen,
    windows: &[Option<Window>; MAX_WINDOWS],
    focus: u64,
    font: &Font,
    seat: &Seat,
) {
    if let Some(cursor) = seat.pointer.cursor() {
        let (x, y, width, height) =
            pointer::cursor_rect(cursor, screen.width, screen.height, theme::scale());
        compose_region(
            screen,
            windows,
            focus,
            font,
            Some(cursor),
            x,
            y,
            width,
            height,
        );
    }
}

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let Some(framebuffer) = framebuffer::query() else {
        write(b"hk-server-display: no framebuffer\n");
        exit(1)
    };
    if framebuffer.width == 0
        || framebuffer.height == 0
        || framebuffer.stride < framebuffer.width * 4
    {
        write(b"hk-server-display: invalid framebuffer\n");
        exit(1)
    }

    let Some((_mapped, framebuffer_virt)) = framebuffer::map() else {
        write(b"hk-server-display: map failed\n");
        exit(2)
    };

    let width = framebuffer.width as usize;
    let height = framebuffer.height as usize;
    let bytes = width * height * 4;
    let Some((_back_handle, _back_region, back_virt)) =
        MemoryClient::new(endpoint).map(CapabilityKind::Surface, bytes)
    else {
        write(b"hk-server-display: back buffer failed\n");
        exit(2)
    };
    let screen = Screen::new(
        back_virt as *mut u32,
        framebuffer_virt as *mut u32,
        width,
        height,
        framebuffer.stride as usize / 4,
    );
    let mut theme_buffer = [0_u8; 128];
    let theme_name = theme::select(hk_server_api::boot_cmdline(&mut theme_buffer));
    let font = hk_canvas::default_font_scaled(theme::scale());
    write(b"hk-server-display: font ");
    write(if font.has_scalable() {
        b"scalable ".as_slice()
    } else {
        b"bitmap ".as_slice()
    });
    write_u64(u64::from(font.size()));
    write(b"\n");
    write(b"hk-server-display: theme ");
    write(theme_name.as_bytes());
    write(b"\n");
    write(b"hk-server-display: scale ");
    write_u64(theme::scale() as u64);
    write(b"\n");
    draw_desktop(&screen);
    write(b"hk-server-display: ready ");
    write_u64(u64::from(framebuffer.width));
    write(b"x");
    write_u64(u64::from(framebuffer.height));
    write(b"\n");

    let mut windows: [Option<Window>; MAX_WINDOWS] = [None; MAX_WINDOWS];
    let mut surfaces: [Option<Surface>; MAX_SURFACES] = [None; MAX_SURFACES];
    let mut next_window_id = 1_u64;
    let mut next_surface_id = 1_u64;
    let mut focus = 0_u64;
    let mut driver_input = false;
    let mut seat = Seat::new();
    let mut drag: Option<Drag> = None;
    let mut resize: Option<Resize> = None;
    damage_taskbar(&screen, &windows, focus, font, seat.pointer.cursor());

    let mut benchmarked = false;
    let benchmark_at = tsc().saturating_add(tsc_frequency().saturating_mul(2));

    loop {
        screen.present();
        if !benchmarked && tsc() >= benchmark_at {
            benchmarked = true;
            // 空闲后连续测量，取最小值，排除启动期抢占等待带来的抖动。
            let mut best = u64::MAX;
            for _ in 0..3 {
                best = best.min(measure_full_redraw(
                    &screen,
                    &windows,
                    focus,
                    font,
                    seat.pointer.cursor(),
                ));
            }
            report_full_redraw(b"full redraw min ", best);
        }
        // 输入驱动接入后不再读内核控制台，避免两路输入重复。
        if !driver_input {
            while let Some(byte) = console_read() {
                if byte == TAB {
                    cycle_focus(
                        &screen,
                        &mut windows,
                        &mut focus,
                        font,
                        seat.pointer.cursor(),
                    );
                } else {
                    route_key(&windows, focus, byte);
                }
            }
        }
        let mut payload = [0_u8; 8 + TITLE_MAX];
        let (label, word, sender, length, handle) = recv_data_handle_try(endpoint, &mut payload, 0);
        if label == 0 {
            let _ = yield_now();
            continue;
        }
        match label {
            SURFACE_CREATE => {
                let (width, height) = unpack_size(word);
                let surface = create_surface(
                    &windows,
                    &mut surfaces,
                    &mut next_surface_id,
                    sender,
                    width,
                    height,
                    handle,
                );
                let _ = send(sender, SURFACE_CREATED, surface.unwrap_or(0));
            }
            WINDOW_CREATE => {
                let window = attach_window(
                    &screen,
                    &mut windows,
                    &mut surfaces,
                    word,
                    &mut next_window_id,
                    &mut focus,
                    font,
                    seat.pointer.cursor(),
                );
                let _ = send(sender, WINDOW_CREATED, window.unwrap_or(0));
            }
            WINDOW_RESIZE => {
                let (id, width, height) = unpack_window_size(word);
                replace_window_surface(
                    &screen,
                    &mut windows,
                    focus,
                    font,
                    seat.pointer.cursor(),
                    id,
                    width as usize,
                    height as usize,
                    handle,
                    sender,
                );
            }
            SURFACE_DAMAGE => {
                if let Some((surface, x, y, width, height)) =
                    unpack_surface_rect(&payload[..length.min(payload.len())])
                {
                    let _ = mark_surface_damage(
                        &mut windows,
                        &mut surfaces,
                        surface,
                        x as usize,
                        y as usize,
                        width as usize,
                        height as usize,
                    );
                }
            }
            SURFACE_SET_INPUT_REGION => {
                if let Some((surface, x, y, width, height)) =
                    unpack_surface_rect(&payload[..length.min(payload.len())])
                {
                    let region = Rect {
                        x: x as usize,
                        y: y as usize,
                        width: width as usize,
                        height: height as usize,
                    };
                    let _ = set_surface_input_region(&mut windows, &mut surfaces, surface, region);
                }
            }
            SURFACE_SET_CURSOR => {
                let (surface, shape) = unpack_surface_cursor(word);
                if let Some(shape) = shape {
                    let found =
                        pointer::set_surface_cursor(&mut windows, &mut surfaces, surface, shape);
                    write(b"hk-server-display: cursor hint ");
                    write_u64(surface);
                    write(b" ");
                    write(pointer::label(shape).as_bytes());
                    write(if found {
                        b"\n".as_slice()
                    } else {
                        b" missed\n".as_slice()
                    });
                    if found && let Some((x, y)) = seat.pointer.position() {
                        let hover = pointer::hover_at(&windows, &screen, x, y);
                        let shape = pointer::shape_for_hover(hover, &windows);
                        apply_hover(&screen, &windows, focus, font, &mut seat, hover, shape);
                        redraw_cursor(&screen, &windows, focus, font, &seat);
                    }
                }
            }
            SURFACE_COMMIT => {
                let slot = windows.iter().position(|window| {
                    window
                        .as_ref()
                        .is_some_and(|window| window.surface.id == word)
                });
                let Some(slot) = slot else {
                    if let Some(surface) = surfaces
                        .iter_mut()
                        .flatten()
                        .find(|surface| surface.id == word)
                    {
                        let _ = surface.take_damage();
                    }
                    let _ = send(sender, SURFACE_FRAME_DONE, word);
                    continue;
                };
                let damage = windows[slot]
                    .as_mut()
                    .and_then(|window| window.surface.take_damage());
                let Some(window) = windows[slot] else {
                    continue;
                };
                match damage {
                    Some(damage) => {
                        let body = clip(
                            damage.x,
                            damage.y,
                            damage.width,
                            damage.height,
                            window.width,
                            window.height,
                        );
                        if let Some(body) = body {
                            compose_region(
                                &screen,
                                &windows,
                                focus,
                                font,
                                seat.pointer.cursor(),
                                window.x + body.x,
                                window.y + title_height() + body.y,
                                body.width,
                                body.height,
                            );
                        }
                        write(b"hk-server-display: surface ");
                        write_u64(word);
                        write(b" ok\n");
                    }
                    None => {
                        damage_window(
                            &screen,
                            &windows,
                            focus,
                            font,
                            seat.pointer.cursor(),
                            &window,
                        );
                        let expected = surface_pixel(&window, 0, 0);
                        let actual = screen.get(window.x, window.y + title_height());
                        if expected != actual {
                            write(b"hk-server-display: composite mismatch\n");
                        } else {
                            write(b"hk-server-display: composite ok ");
                            write_u64(window.id);
                            write(b" ");
                            write_u64(window.width as u64);
                            write(b"x");
                            write_u64(window.height as u64);
                            write(b"\n");
                        }
                    }
                }
                let _ = send(sender, SURFACE_FRAME_DONE, word);
                if let Some(window) = windows[slot].as_ref()
                    && (window.width != window.surface.width
                        || window.height != window.surface.height)
                {
                    let _ = send(
                        window.surface.client,
                        DISPLAY_RESIZE,
                        pack_window_size(window.id, window.width as u32, window.height as u32),
                    );
                }
            }
            SURFACE_DESTROY => {
                discard_surface(&mut surfaces, word);
                if let Some(window) = windows
                    .iter()
                    .flatten()
                    .find(|window| window.surface.id == word)
                    .copied()
                {
                    close_window(
                        &screen,
                        &mut windows,
                        &mut focus,
                        font,
                        seat.pointer.cursor(),
                        window.id,
                    );
                }
            }
            SEAT_FOCUS => {
                let id = word;
                if windows.iter().flatten().any(|window| window.id == id) {
                    let previous = focus;
                    raise(&mut windows, id);
                    focus = id;
                    if let Some(window) = windows
                        .iter()
                        .flatten()
                        .find(|window| window.id == id)
                        .copied()
                    {
                        damage_window(
                            &screen,
                            &windows,
                            focus,
                            font,
                            seat.pointer.cursor(),
                            &window,
                        );
                    }
                    damage_title(
                        &screen,
                        &windows,
                        focus,
                        font,
                        seat.pointer.cursor(),
                        previous,
                    );
                    damage_taskbar(&screen, &windows, focus, font, seat.pointer.cursor());
                    write(b"hk-server-display: focus ");
                    write_u64(id);
                    write(b"\n");
                }
            }
            SEAT_SET_CLIPBOARD => {
                let text = &payload[..length.min(payload.len())];
                if seat.set_clipboard(sender, text) {
                    write(b"hk-server-display: clipboard set ");
                    write_u64(sender);
                    write(b"\n");
                }
            }
            SEAT_GET_CLIPBOARD => {
                let text = seat.clipboard();
                let _ = send_data(sender, SEAT_CLIPBOARD, text);
                write(b"hk-server-display: clipboard get ");
                write_u64(seat.clipboard_owner());
                write(b" ");
                write_u64(text.len() as u64);
                write(b"\n");
            }
            INPUT_POINTER => {
                static FIRST_POINTER: core::sync::atomic::AtomicBool =
                    core::sync::atomic::AtomicBool::new(true);
                if FIRST_POINTER.swap(false, core::sync::atomic::Ordering::Relaxed) {
                    write(b"hk-server-display: pointer ready\n");
                }
                let (x, y) = unpack_pointer(word);
                let next = pointer::normalize(x, y, screen.width, screen.height);
                erase_cursor(&screen, &windows, focus, font, seat.pointer.cursor());
                if let Some(state) = resize
                    && let Some(window) = windows
                        .iter()
                        .flatten()
                        .find(|window| window.id == state.id)
                        .copied()
                {
                    let desktop_height = screen.height.saturating_sub(taskbar_height());
                    let delta_x = next.0 as isize - state.start_x as isize;
                    let delta_y = next.1 as isize - state.start_y as isize;
                    let next_rect = resize::resized(
                        state.original,
                        state.edge,
                        delta_x,
                        delta_y,
                        screen.width,
                        desktop_height,
                        title_height(),
                        MIN_WINDOW_WIDTH,
                        MIN_WINDOW_HEIGHT,
                    );
                    if (next_rect.x, next_rect.y, next_rect.width, next_rect.height)
                        != (window.x, window.y, window.width, window.height)
                    {
                        resize_window(
                            &screen,
                            &mut windows,
                            focus,
                            font,
                            window,
                            next_rect.x,
                            next_rect.y,
                            next_rect.width,
                            next_rect.height,
                        );
                        if !state.resized {
                            resize = Some(Resize {
                                resized: true,
                                ..state
                            });
                        }
                    }
                } else if let Some(state) = drag
                    && let Some(window) = windows
                        .iter()
                        .flatten()
                        .find(|window| window.id == state.id)
                        .copied()
                {
                    let max_x = screen.width.saturating_sub(window.width);
                    let desktop_height = screen.height.saturating_sub(taskbar_height());
                    let max_y = desktop_height.saturating_sub(window.height + title_height());
                    let new_x = next.0.saturating_sub(state.offset_x).min(max_x);
                    let new_y = next.1.saturating_sub(state.offset_y).min(max_y);
                    if (new_x, new_y) != (window.x, window.y) {
                        move_window(&screen, &mut windows, focus, font, window, new_x, new_y);
                        if !state.moved {
                            drag = Some(Drag {
                                moved: true,
                                ..state
                            });
                            write(b"hk-server-display: drag ");
                            write_u64(state.id);
                            write(b" ");
                            write_u64(new_x as u64);
                            write(b",");
                            write_u64(new_y as u64);
                            write(b"\n");
                        }
                    }
                }
                seat.pointer.set_position(next);
                let hover = if drag.is_some() || resize.is_some() {
                    Hover::None
                } else {
                    pointer::hover_at(&windows, &screen, next.0, next.1)
                };
                let shape = if let Some(state) = resize {
                    pointer::shape_for_edges(state.edge)
                } else if drag.is_some() {
                    Shape::Move
                } else {
                    pointer::shape_for_hover(hover, &windows)
                };
                apply_hover(&screen, &windows, focus, font, &mut seat, hover, shape);
                redraw_cursor(&screen, &windows, focus, font, &seat);
                seat.route_pointer(&windows, hover, next.0, next.1);
            }
            INPUT_BUTTON => {
                if word == 0 {
                    seat.pointer.set_pressed(false);
                    if let Some(state) = resize
                        && state.resized
                    {
                        if let Some(window) = windows
                            .iter()
                            .flatten()
                            .find(|window| window.id == state.id)
                            .copied()
                        {
                            write(b"hk-server-display: resize ");
                            write_u64(window.id);
                            write(b" ");
                            write_u64(window.width as u64);
                            write(b"x");
                            write_u64(window.height as u64);
                            write(b"\n");
                        }
                        notify_resize(&windows, state.id);
                    }
                    drag = None;
                    resize = None;
                    if let Some((x, y)) = seat.pointer.position() {
                        let hover = pointer::hover_at(&windows, &screen, x, y);
                        let shape = pointer::shape_for_hover(hover, &windows);
                        apply_hover(&screen, &windows, focus, font, &mut seat, hover, shape);
                    }
                    redraw_cursor(&screen, &windows, focus, font, &seat);
                    continue;
                }
                let was_pressed = seat.pointer.pressed();
                seat.pointer.set_pressed(true);
                drag = None;
                resize = None;
                if let Some((x, y)) = seat.pointer.position() {
                    let hover = pointer::hover_at(&windows, &screen, x, y);
                    if !was_pressed {
                        let clicks = seat.pointer.register_press(hover, tsc(), tsc_frequency());
                        if clicks == 2
                            && let Hover::Title(id) = hover
                        {
                            toggle_maximize(
                                &screen,
                                &mut windows,
                                focus,
                                font,
                                seat.pointer.cursor(),
                                id,
                            );
                            write(b"hk-server-display: double click ");
                            write_u64(id);
                            write(b"\n");
                            let hover = pointer::hover_at(&windows, &screen, x, y);
                            let shape = pointer::shape_for_hover(hover, &windows);
                            apply_hover(&screen, &windows, focus, font, &mut seat, hover, shape);
                            redraw_cursor(&screen, &windows, focus, font, &seat);
                            continue;
                        }
                    }
                    if y >= taskbar_top(screen.height) {
                        if let Some(id) = taskbar_at(&windows, x, y, screen.width, screen.height) {
                            let old_focus = focus;
                            if let Some(window) =
                                windows.iter_mut().flatten().find(|window| window.id == id)
                            {
                                window.minimized = false;
                                set_scanout(window.surface.handle, window.surface.id, true);
                            }
                            raise(&mut windows, id);
                            focus = id;
                            if let Some(window) = windows
                                .iter()
                                .flatten()
                                .find(|window| window.id == id)
                                .copied()
                            {
                                damage_window(
                                    &screen,
                                    &windows,
                                    focus,
                                    font,
                                    seat.pointer.cursor(),
                                    &window,
                                );
                            }
                            damage_title(
                                &screen,
                                &windows,
                                focus,
                                font,
                                seat.pointer.cursor(),
                                old_focus,
                            );
                            damage_taskbar(&screen, &windows, focus, font, seat.pointer.cursor());
                            redraw_cursor(&screen, &windows, focus, font, &seat);
                            write(b"hk-server-display: taskbar ");
                            write_u64(id);
                            write(b"\n");
                        }
                        continue;
                    }
                    let old_focus = focus;
                    if let Some(window) = window_at(&windows, x, y) {
                        if close_at(&window, x, y) {
                            close_window(
                                &screen,
                                &mut windows,
                                &mut focus,
                                font,
                                seat.pointer.cursor(),
                                window.id,
                            );
                            continue;
                        }
                        if minimize_at(&window, x, y) {
                            minimize_window(
                                &screen,
                                &mut windows,
                                &mut focus,
                                font,
                                seat.pointer.cursor(),
                                window.id,
                            );
                            continue;
                        }
                        if maximize_at(&window, x, y) {
                            toggle_maximize(
                                &screen,
                                &mut windows,
                                focus,
                                font,
                                seat.pointer.cursor(),
                                window.id,
                            );
                            continue;
                        }
                        let id = window.id;
                        raise(&mut windows, id);
                        focus = id;
                        if let Some(window) = windows
                            .iter()
                            .flatten()
                            .find(|window| window.id == id)
                            .copied()
                        {
                            damage_window(
                                &screen,
                                &windows,
                                focus,
                                font,
                                seat.pointer.cursor(),
                                &window,
                            );
                        }
                        damage_title(
                            &screen,
                            &windows,
                            focus,
                            font,
                            seat.pointer.cursor(),
                            old_focus,
                        );
                        damage_taskbar(&screen, &windows, focus, font, seat.pointer.cursor());
                        redraw_cursor(&screen, &windows, focus, font, &seat);
                        let on_title = y >= window.y && y < window.y + title_height();
                        if let Some(edge) = resize_at(&window, x, y) {
                            resize = Some(Resize {
                                id,
                                edge,
                                start_x: x,
                                start_y: y,
                                original: resize::Rect {
                                    x: window.x,
                                    y: window.y,
                                    width: window.width,
                                    height: window.height,
                                },
                                resized: false,
                            });
                        } else if on_title {
                            drag = Some(Drag {
                                id,
                                offset_x: x - window.x,
                                offset_y: y - window.y,
                                moved: false,
                            });
                        } else {
                            write(b"hk-server-display: pointer click ");
                            write_u64(id);
                            write(b"\n");
                            if let Some(window) =
                                windows.iter().flatten().find(|window| window.id == id)
                            {
                                let _ = send(window.surface.client, DISPLAY_CLICK, id);
                                let body_x = x.saturating_sub(window.x) as u32;
                                let body_y = y.saturating_sub(window.y + title_height()) as u32;
                                let _ = send(
                                    window.surface.client,
                                    DISPLAY_CLICK_AT,
                                    pack_click_at(id, body_x, body_y),
                                );
                            }
                        }
                    }
                }
            }
            INPUT_KEY => {
                driver_input = true;
                let byte = word as u8;
                if byte == TAB {
                    cycle_focus(
                        &screen,
                        &mut windows,
                        &mut focus,
                        font,
                        seat.pointer.cursor(),
                    );
                } else {
                    route_key(&windows, focus, byte);
                }
            }
            INPUT_WHEEL => {
                let delta = unpack_wheel(word);
                if delta != 0 {
                    route_wheel(&windows, focus, delta);
                }
            }
            DISPLAY_SET_TITLE => {
                if length >= 8 {
                    let id = u64::from_le_bytes([
                        payload[0], payload[1], payload[2], payload[3], payload[4], payload[5],
                        payload[6], payload[7],
                    ]);
                    let text = &payload[8..length];
                    if let Some(window) =
                        windows.iter_mut().flatten().find(|window| window.id == id)
                    {
                        let mut count = text.len().min(TITLE_MAX);
                        while count > 0 && core::str::from_utf8(&text[..count]).is_err() {
                            count -= 1;
                        }
                        window.title[..count].copy_from_slice(&text[..count]);
                        window.title_length = count;
                    }
                    damage_title(&screen, &windows, focus, font, seat.pointer.cursor(), id);
                    damage_taskbar(&screen, &windows, focus, font, seat.pointer.cursor());
                    let titled = windows
                        .iter()
                        .flatten()
                        .find(|window| window.id == id)
                        .is_some_and(|window| {
                            let palette = theme::theme();
                            let color = if window.id == focus {
                                palette.title_focus
                            } else {
                                palette.title_color
                            };
                            (window.y + 4..(window.y + title_height()).min(screen.height)).any(
                                |row| {
                                    (window.x + 4..(window.x + window.width).min(screen.width))
                                        .any(|column| screen.get(column, row) != color)
                                },
                            )
                        });
                    if titled {
                        write(b"hk-server-display: title ok\n");
                    }
                }
            }
            DISPLAY_SHUTDOWN => {
                for window in windows.iter().flatten() {
                    let _ = close_handle(window.surface.handle);
                }
                write(b"hk-server-display: bye\n");
                exit(0)
            }
            DISPLAY_RESTART => {
                write(b"hk-server-display: restart requested\n");
                exit(1)
            }
            _ => {}
        }
    }
}

hk_server_api::server_boilerplate!();
