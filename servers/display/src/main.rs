//! display：帧缓冲合成器与窗口服务。
//!
//! 只有本服务持有帧缓冲授权。客户端把共享 Frame 作为窗口表面，经句柄转移
//! 交给本服务；合成按区域重绘，避免每次提交都重画整屏。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use hk_server_api::display::{
    DISPLAY_CLICK, DISPLAY_COMMIT, DISPLAY_CREATE_WINDOW, DISPLAY_FRAME_DONE, DISPLAY_KEY,
    DISPLAY_SET_TITLE, DISPLAY_SHUTDOWN, DISPLAY_WINDOW_CREATED, TITLE_MAX, pack_key, unpack_size,
};
#[cfg(target_os = "none")]
use hk_server_api::framebuffer;
#[cfg(target_os = "none")]
use hk_server_api::input::{INPUT_BUTTON, INPUT_KEY, INPUT_POINTER, unpack_pointer};
#[cfg(target_os = "none")]
use hk_server_api::{
    close_handle, console_read, exit, frame_info, frame_map, recv_data_handle_try, send, write,
    write_u64, yield_now,
};
#[cfg(target_os = "none")]
use lovyangfx_fonts::Font;
#[cfg(target_os = "none")]
use lovyangfx_fonts::fonts::efont_cn_16;

#[cfg(target_os = "none")]
mod compositor;
#[cfg(target_os = "none")]
use compositor::{
    CURSOR_HEIGHT, CURSOR_WIDTH, TITLE_COLOR, TITLE_FOCUS, compose_region, damage_taskbar,
    damage_title, damage_window, draw_desktop, move_window, surface_pixel,
};

#[cfg(target_os = "none")]
mod window;
#[cfg(target_os = "none")]
use window::{
    Drag, MAX_WINDOWS, Screen, TASKBAR_HEIGHT, TITLE_HEIGHT, WINDOW_BASE, WINDOW_STRIDE, Window,
    raise, taskbar_at, taskbar_top, window_at,
};

/// Tab 键字节：切换焦点。
#[cfg(target_os = "none")]
const TAB: u8 = b'\t';

/// 把按键转发给焦点窗口的客户端；没有焦点时丢弃。
#[cfg(target_os = "none")]
fn route_key(windows: &[Option<Window>; MAX_WINDOWS], focus: u64, byte: u8) {
    if let Some(window) = windows.iter().flatten().find(|window| window.id == focus) {
        let _ = send(window.client, DISPLAY_KEY, pack_key(window.id, byte));
    }
}

/// Tab：把焦点移到下一个窗口并提升它。
#[cfg(target_os = "none")]
fn cycle_focus(
    screen: &Screen,
    windows: &mut [Option<Window>; MAX_WINDOWS],
    focus: &mut u64,
    font: &Font<'static>,
    pointer: Option<(usize, usize)>,
) {
    let count = windows.iter().flatten().count();
    if count == 0 {
        return;
    }
    let position = windows
        .iter()
        .flatten()
        .position(|window| window.id == *focus)
        .unwrap_or(count - 1);
    let next = (position + 1) % count;
    let Some(id) = windows.iter().flatten().nth(next).map(|window| window.id) else {
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
        damage_window(screen, windows, *focus, font, pointer, &window);
    }
    damage_title(screen, windows, *focus, font, pointer, previous);
    damage_taskbar(screen, windows, *focus, font, pointer);
    write(b"hk-server-display: focus ");
    write_u64(id);
    write(b"\n");
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

    let screen = Screen {
        base: framebuffer::MAP_ADDRESS as *mut u32,
        width: framebuffer.width as usize,
        height: framebuffer.height as usize,
        stride: framebuffer.stride as usize / 4,
    };
    let font = efont_cn_16();
    draw_desktop(&screen);
    write(b"hk-server-display: ready ");
    write_u64(u64::from(framebuffer.width));
    write(b"x");
    write_u64(u64::from(framebuffer.height));
    write(b"\n");

    let mut windows: [Option<Window>; MAX_WINDOWS] = [None; MAX_WINDOWS];
    let mut next_id = 1_u64;
    let mut focus = 0_u64;
    let mut driver_input = false;
    let mut pointer: Option<(usize, usize)> = None;
    let mut drag: Option<Drag> = None;
    damage_taskbar(&screen, &windows, focus, &font, pointer);
    loop {
        // 输入驱动接入后不再读内核控制台，避免两路输入重复。
        if !driver_input {
            while let Some(byte) = console_read() {
                if byte == TAB {
                    cycle_focus(&screen, &mut windows, &mut focus, &font, pointer);
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
            DISPLAY_CREATE_WINDOW => {
                let (width, height) = unpack_size(word);
                if handle == 0 || width == 0 || height == 0 {
                    if handle != 0 {
                        let _ = close_handle(handle);
                    }
                    let _ = send(sender, DISPLAY_WINDOW_CREATED, 0);
                    continue;
                }
                let Some(slot) = windows.iter().position(Option::is_none) else {
                    let _ = close_handle(handle);
                    let _ = send(sender, DISPLAY_WINDOW_CREATED, 0);
                    continue;
                };
                let bytes = width as u64 * height as u64 * 4;
                let Some((_, frame_length)) = frame_info(handle) else {
                    let _ = close_handle(handle);
                    let _ = send(sender, DISPLAY_WINDOW_CREATED, 0);
                    continue;
                };
                if frame_length < bytes {
                    let _ = close_handle(handle);
                    let _ = send(sender, DISPLAY_WINDOW_CREATED, 0);
                    continue;
                }
                let id = next_id;
                let virt = WINDOW_BASE + (slot as u64) * WINDOW_STRIDE;
                if frame_map(handle, virt) < bytes {
                    let _ = close_handle(handle);
                    let _ = send(sender, DISPLAY_WINDOW_CREATED, 0);
                    continue;
                }
                let offset = (id as usize - 1) * 16;
                let width = width as usize;
                let height = height as usize;
                let x = screen
                    .width
                    .saturating_sub(width)
                    .saturating_div(2)
                    .saturating_add(offset)
                    .min(screen.width.saturating_sub(width));
                let desktop_height = screen.height.saturating_sub(TASKBAR_HEIGHT);
                let y = desktop_height
                    .saturating_sub(height + TITLE_HEIGHT)
                    .saturating_div(2)
                    .saturating_add(offset)
                    .min(desktop_height.saturating_sub(height + TITLE_HEIGHT));
                let previous = focus;
                windows[slot] = Some(Window {
                    id,
                    x,
                    y,
                    width,
                    height,
                    virt,
                    handle,
                    client: sender,
                    title: [0; TITLE_MAX],
                    title_length: 0,
                });
                next_id += 1;
                focus = id;
                if let Some(window) = windows[slot] {
                    damage_window(&screen, &windows, focus, &font, pointer, &window);
                }
                damage_title(&screen, &windows, focus, &font, pointer, previous);
                damage_taskbar(&screen, &windows, focus, &font, pointer);
                let _ = send(sender, DISPLAY_WINDOW_CREATED, id);
            }
            INPUT_POINTER => {
                let (x, y) = unpack_pointer(word);
                let next = (
                    (u64::from(x) * screen.width as u64 / 0x7FFF) as usize,
                    (u64::from(y) * screen.height as u64 / 0x7FFF) as usize,
                );
                if let Some(previous) = pointer {
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
                if let Some(state) = drag
                    && let Some(window) = windows
                        .iter()
                        .flatten()
                        .find(|window| window.id == state.id)
                        .copied()
                {
                    let max_x = screen.width.saturating_sub(window.width);
                    let desktop_height = screen.height.saturating_sub(TASKBAR_HEIGHT);
                    let max_y = desktop_height.saturating_sub(window.height + TITLE_HEIGHT);
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
                pointer = Some(next);
                compose_region(
                    &screen,
                    &windows,
                    focus,
                    &font,
                    pointer,
                    next.0,
                    next.1,
                    CURSOR_WIDTH + 1,
                    CURSOR_HEIGHT + 1,
                );
            }
            INPUT_BUTTON => {
                if word == 0 {
                    drag = None;
                    continue;
                }
                drag = None;
                if let Some((x, y)) = pointer {
                    if y >= taskbar_top(screen.height) {
                        if let Some(id) = taskbar_at(&windows, x, y, screen.width, screen.height) {
                            let old_focus = focus;
                            raise(&mut windows, id);
                            focus = id;
                            if let Some(window) = windows
                                .iter()
                                .flatten()
                                .find(|window| window.id == id)
                                .copied()
                            {
                                damage_window(&screen, &windows, focus, &font, pointer, &window);
                            }
                            damage_title(&screen, &windows, focus, &font, pointer, old_focus);
                            damage_taskbar(&screen, &windows, focus, &font, pointer);
                            compose_region(
                                &screen,
                                &windows,
                                focus,
                                &font,
                                pointer,
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
                        let id = window.id;
                        raise(&mut windows, id);
                        focus = id;
                        if let Some(window) = windows
                            .iter()
                            .flatten()
                            .find(|window| window.id == id)
                            .copied()
                        {
                            damage_window(&screen, &windows, focus, &font, pointer, &window);
                        }
                        damage_title(&screen, &windows, focus, &font, pointer, old_focus);
                        damage_taskbar(&screen, &windows, focus, &font, pointer);
                        compose_region(
                            &screen,
                            &windows,
                            focus,
                            &font,
                            pointer,
                            x,
                            y,
                            CURSOR_WIDTH + 1,
                            CURSOR_HEIGHT + 1,
                        );
                        let on_title = y >= window.y && y < window.y + TITLE_HEIGHT;
                        if on_title {
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
                                let _ = send(window.client, DISPLAY_CLICK, id);
                            }
                        }
                    }
                }
            }
            INPUT_KEY => {
                driver_input = true;
                let byte = word as u8;
                if byte == TAB {
                    cycle_focus(&screen, &mut windows, &mut focus, &font, pointer);
                } else {
                    route_key(&windows, focus, byte);
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
                        let count = text.len().min(TITLE_MAX);
                        window.title[..count].copy_from_slice(&text[..count]);
                        window.title_length = count;
                    }
                    damage_title(&screen, &windows, focus, &font, pointer, id);
                    damage_taskbar(&screen, &windows, focus, &font, pointer);
                    let titled = windows
                        .iter()
                        .flatten()
                        .find(|window| window.id == id)
                        .is_some_and(|window| {
                            let color = if window.id == focus {
                                TITLE_FOCUS
                            } else {
                                TITLE_COLOR
                            };
                            (window.y + 4..(window.y + TITLE_HEIGHT).min(screen.height)).any(
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
            DISPLAY_COMMIT => {
                let Some(window) = windows
                    .iter()
                    .flatten()
                    .find(|window| window.id == word)
                    .copied()
                else {
                    continue;
                };
                raise(&mut windows, word);
                let previous = focus;
                focus = window.id;
                let Some(window) = windows
                    .iter()
                    .flatten()
                    .find(|window| window.id == word)
                    .copied()
                else {
                    continue;
                };
                damage_window(&screen, &windows, focus, &font, pointer, &window);
                damage_title(&screen, &windows, focus, &font, pointer, previous);
                damage_taskbar(&screen, &windows, focus, &font, pointer);
                let expected = surface_pixel(&window, 0, 0);
                let actual = screen.get(window.x, window.y + TITLE_HEIGHT);
                if expected != actual {
                    write(b"hk-server-display: composite mismatch\n");
                    let _ = send(sender, DISPLAY_FRAME_DONE, window.id);
                    continue;
                }
                write(b"hk-server-display: composite ok ");
                write_u64(window.id);
                write(b" ");
                write_u64(window.width as u64);
                write(b"x");
                write_u64(window.height as u64);
                write(b"\n");
                let _ = send(sender, DISPLAY_FRAME_DONE, window.id);
            }
            DISPLAY_SHUTDOWN => {
                for window in windows.iter().flatten() {
                    let _ = close_handle(window.handle);
                }
                write(b"hk-server-display: bye\n");
                exit(0)
            }
            _ => {}
        }
    }
}

hk_server_api::server_boilerplate!();
