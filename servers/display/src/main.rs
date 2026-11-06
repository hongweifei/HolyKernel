//! display：帧缓冲合成器与窗口服务。
//!
//! 只有本服务持有帧缓冲授权。客户端把共享 Frame 作为窗口表面，经句柄转移
//! 交给本服务；合成按区域重绘，避免每次提交都重画整屏。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(any(target_os = "none", test))]
mod resize;
#[cfg(any(target_os = "none", test))]
mod seat;
#[cfg(any(target_os = "none", test))]
mod surface;

#[cfg(target_os = "none")]
use hk_font::Font;
#[cfg(target_os = "none")]
use hk_server_api::display::{
    DISPLAY_CLICK, DISPLAY_CLICK_AT, DISPLAY_RESTART, DISPLAY_SET_TITLE, DISPLAY_SHUTDOWN,
    SEAT_CLIPBOARD, SEAT_FOCUS, SEAT_GET_CLIPBOARD, SEAT_SET_CLIPBOARD, SURFACE_COMMIT,
    SURFACE_CREATE, SURFACE_CREATED, SURFACE_DAMAGE, SURFACE_DESTROY, SURFACE_FRAME_DONE,
    SURFACE_SET_INPUT_REGION, TITLE_MAX, WINDOW_CREATE, WINDOW_CREATED, pack_click_at, unpack_size,
    unpack_surface_rect,
};
#[cfg(target_os = "none")]
use hk_server_api::framebuffer;
#[cfg(target_os = "none")]
use hk_server_api::input::{
    INPUT_BUTTON, INPUT_KEY, INPUT_POINTER, INPUT_WHEEL, unpack_pointer, unpack_wheel,
};
#[cfg(target_os = "none")]
use hk_server_api::{
    close_handle, console_read, exit, frame_alloc, frame_info, frame_map, frame_unmap,
    recv_data_handle_try, send, send_data, tsc, tsc_frequency, write, write_u64, yield_now,
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
    CURSOR_HEIGHT, CURSOR_WIDTH, compose_region, damage_taskbar, damage_title, damage_window,
    draw_desktop, move_window, resize_window, surface_pixel,
};

#[cfg(target_os = "none")]
use resize::Rect;
#[cfg(target_os = "none")]
use seat::Seat;
#[cfg(target_os = "none")]
use surface::{MAX_SURFACES, Surface, scale};

#[cfg(target_os = "none")]
mod theme;
#[cfg(target_os = "none")]
mod window;
#[cfg(target_os = "none")]
use window::{
    Drag, MAX_WINDOWS, MIN_WINDOW_HEIGHT, MIN_WINDOW_WIDTH, Resize, Screen, WINDOW_BASE,
    WINDOW_STRIDE, Window, close_at, maximize_at, minimize_at, raise, resize_at, taskbar_at,
    taskbar_height, taskbar_top, title_height, window_at,
};

/// Tab 键字节：切换焦点。
#[cfg(target_os = "none")]
const TAB: u8 = b'\t';

/// 离屏后备缓冲的映射地址。
#[cfg(target_os = "none")]
const BACK_VIRT: u64 = 0x7000_0000;

/// 把输入的 0..=0x7FFF 绝对坐标映射到屏幕像素范围。
///
/// 最大值映射到 `width - 1` / `height - 1`，避免指针落在屏幕外一像素；空屏幕
/// 时退化为 (0, 0)。
#[cfg(target_os = "none")]
fn pointer_position(x: u16, y: u16, width: usize, height: usize) -> (usize, usize) {
    let limit_x = width.saturating_sub(1) as u64;
    let limit_y = height.saturating_sub(1) as u64;
    (
        (u64::from(x) * width as u64 / 0x7FFF).min(limit_x) as usize,
        (u64::from(y) * height as u64 / 0x7FFF).min(limit_y) as usize,
    )
}

/// 全屏合成并提交，返回耗时 cycles。
#[cfg(target_os = "none")]
fn measure_full_redraw(
    screen: &Screen,
    windows: &[Option<Window>; MAX_WINDOWS],
    focus: u64,
    font: &Font,
    pointer: Option<(usize, usize)>,
) -> u64 {
    let start = tsc();
    compose_region(
        screen,
        windows,
        focus,
        font,
        pointer,
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
    let Some((_, frame_length)) = frame_info(handle) else {
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
    let virt = WINDOW_BASE + (slot as u64) * WINDOW_STRIDE;
    let mapped = frame_map(handle, virt);
    if mapped < bytes {
        let _ = close_handle(handle);
        write(b"hk-server-display: create map-failed ");
        write_u64(mapped);
        write(b"/");
        write_u64(bytes);
        write(b"\n");
        return None;
    }
    surfaces[slot] = Some(Surface::new(id, client, slot, width, height, virt, handle));
    *next_id += 1;
    Some(id)
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
        let bytes = surface
            .width
            .saturating_mul(surface.height)
            .saturating_mul(4);
        let _ = frame_unmap(surface.virt, bytes);
        let _ = close_handle(surface.handle);
    }
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
    pointer: Option<(usize, usize)>,
) -> Option<u64> {
    let slot = surfaces.iter().position(|entry| {
        entry
            .as_ref()
            .is_some_and(|surface| surface.id == surface_id)
    })?;
    let Some(surface) = surfaces[slot].take() else {
        return None;
    };
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
        damage_window(screen, windows, *focus, font, pointer, &window);
    }
    damage_title(screen, windows, *focus, font, pointer, previous);
    damage_taskbar(screen, windows, *focus, font, pointer);
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

    let mapped = framebuffer::map(framebuffer::MAP_ADDRESS);
    if mapped == 0 {
        write(b"hk-server-display: map failed\n");
        exit(2)
    }

    let width = framebuffer.width as usize;
    let height = framebuffer.height as usize;
    let bytes = width * height * 4;
    let back = frame_alloc(bytes, BACK_VIRT);
    if back == 0 || back == u64::MAX {
        write(b"hk-server-display: back buffer failed\n");
        exit(2)
    }
    let screen = Screen::new(
        BACK_VIRT as *mut u32,
        framebuffer::MAP_ADDRESS as *mut u32,
        width,
        height,
        framebuffer.stride as usize / 4,
    );
    let mut theme_buffer = [0_u8; 128];
    let theme_name = theme::select(hk_server_api::boot_cmdline(&mut theme_buffer));
    let font = Font::new((16 * theme::scale()) as u32);
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
    damage_taskbar(&screen, &windows, focus, &font, seat.pointer);

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
                    &font,
                    seat.pointer,
                ));
            }
            report_full_redraw(b"full redraw min ", best);
        }
        // 输入驱动接入后不再读内核控制台，避免两路输入重复。
        if !driver_input {
            while let Some(byte) = console_read() {
                if byte == TAB {
                    cycle_focus(&screen, &mut windows, &mut focus, &font, seat.pointer);
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
                    &font,
                    seat.pointer,
                );
                let _ = send(sender, WINDOW_CREATED, window.unwrap_or(0));
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
                        let body = scale(
                            damage,
                            window.surface.width,
                            window.surface.height,
                            window.width,
                            window.height,
                        );
                        if body.width > 0 && body.height > 0 {
                            compose_region(
                                &screen,
                                &windows,
                                focus,
                                &font,
                                seat.pointer,
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
                        damage_window(&screen, &windows, focus, &font, seat.pointer, &window);
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
                        &font,
                        seat.pointer,
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
                        damage_window(&screen, &windows, focus, &font, seat.pointer, &window);
                    }
                    damage_title(&screen, &windows, focus, &font, seat.pointer, previous);
                    damage_taskbar(&screen, &windows, focus, &font, seat.pointer);
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
                let (x, y) = unpack_pointer(word);
                let next = pointer_position(x, y, screen.width, screen.height);
                if let Some(previous) = seat.pointer {
                    compose_region(
                        &screen,
                        &windows,
                        focus,
                        &font,
                        None,
                        previous.0,
                        previous.1,
                        CURSOR_WIDTH + 1,
                        CURSOR_HEIGHT + 1,
                    );
                }
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
                            &font,
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
                        move_window(&screen, &mut windows, focus, &font, window, new_x, new_y);
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
                seat.pointer = Some(next);
                compose_region(
                    &screen,
                    &windows,
                    focus,
                    &font,
                    seat.pointer,
                    next.0,
                    next.1,
                    CURSOR_WIDTH + 1,
                    CURSOR_HEIGHT + 1,
                );
            }
            INPUT_BUTTON => {
                if word == 0 {
                    if let Some(state) = resize
                        && state.resized
                        && let Some(window) = windows
                            .iter()
                            .flatten()
                            .find(|window| window.id == state.id)
                    {
                        write(b"hk-server-display: resize ");
                        write_u64(window.id);
                        write(b" ");
                        write_u64(window.width as u64);
                        write(b"x");
                        write_u64(window.height as u64);
                        write(b"\n");
                    }
                    drag = None;
                    resize = None;
                    continue;
                }
                drag = None;
                resize = None;
                if let Some((x, y)) = seat.pointer {
                    if y >= taskbar_top(screen.height) {
                        if let Some(id) = taskbar_at(&windows, x, y, screen.width, screen.height) {
                            let old_focus = focus;
                            if let Some(window) =
                                windows.iter_mut().flatten().find(|window| window.id == id)
                            {
                                window.minimized = false;
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
                                    &font,
                                    seat.pointer,
                                    &window,
                                );
                            }
                            damage_title(&screen, &windows, focus, &font, seat.pointer, old_focus);
                            damage_taskbar(&screen, &windows, focus, &font, seat.pointer);
                            compose_region(
                                &screen,
                                &windows,
                                focus,
                                &font,
                                seat.pointer,
                                x,
                                y,
                                CURSOR_WIDTH + 1,
                                CURSOR_HEIGHT + 1,
                            );
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
                                &font,
                                seat.pointer,
                                window.id,
                            );
                            continue;
                        }
                        if minimize_at(&window, x, y) {
                            minimize_window(
                                &screen,
                                &mut windows,
                                &mut focus,
                                &font,
                                seat.pointer,
                                window.id,
                            );
                            continue;
                        }
                        if maximize_at(&window, x, y) {
                            toggle_maximize(
                                &screen,
                                &mut windows,
                                focus,
                                &font,
                                seat.pointer,
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
                            damage_window(&screen, &windows, focus, &font, seat.pointer, &window);
                        }
                        damage_title(&screen, &windows, focus, &font, seat.pointer, old_focus);
                        damage_taskbar(&screen, &windows, focus, &font, seat.pointer);
                        compose_region(
                            &screen,
                            &windows,
                            focus,
                            &font,
                            seat.pointer,
                            x,
                            y,
                            CURSOR_WIDTH + 1,
                            CURSOR_HEIGHT + 1,
                        );
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
                    cycle_focus(&screen, &mut windows, &mut focus, &font, seat.pointer);
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
                    damage_title(&screen, &windows, focus, &font, seat.pointer, id);
                    damage_taskbar(&screen, &windows, focus, &font, seat.pointer);
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
