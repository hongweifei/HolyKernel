//! display：帧缓冲合成器与窗口服务。
//!
//! 只有本服务持有帧缓冲授权。客户端把共享 Frame 作为窗口表面，经句柄转移
//! 交给本服务；合成按区域重绘，避免每次提交都重画整屏。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;
#[cfg(target_os = "none")]
use hk_server_api::display::{
    DISPLAY_COMMIT, DISPLAY_CREATE_WINDOW, DISPLAY_FRAME_DONE, DISPLAY_KEY, DISPLAY_SET_TITLE,
    DISPLAY_SHUTDOWN, DISPLAY_WINDOW_CREATED, TITLE_MAX, pack_key, unpack_size,
};
#[cfg(target_os = "none")]
use hk_server_api::framebuffer;
#[cfg(target_os = "none")]
use hk_server_api::{
    close_handle, console_read, exit, frame_info, frame_map, recv_data_handle_try, send, write,
    write_u64, yield_now,
};
#[cfg(target_os = "none")]
use lovyangfx_fonts::Font;
#[cfg(target_os = "none")]
use lovyangfx_fonts::fonts::efont_cn_16;

/// 最多同时合成的窗口数。
#[cfg(target_os = "none")]
const MAX_WINDOWS: usize = 8;

/// 窗口表面映射基址。
#[cfg(target_os = "none")]
const WINDOW_BASE: u64 = 0x6000_0000;

/// 相邻窗口表面的地址间隔。
#[cfg(target_os = "none")]
const WINDOW_STRIDE: u64 = 0x20_0000;

/// 标题栏高度。
#[cfg(target_os = "none")]
const TITLE_HEIGHT: usize = 24;

/// Tab 键字节：切换焦点。
#[cfg(target_os = "none")]
const TAB: u8 = b'\t';

/// 桌面背景顶部颜色。
#[cfg(target_os = "none")]
const DESKTOP_TOP: u32 = 0x0010_1820;
/// 桌面背景底部颜色。
#[cfg(target_os = "none")]
const DESKTOP_BOTTOM: u32 = 0x0030_4860;
/// 标题栏颜色。
#[cfg(target_os = "none")]
const TITLE_COLOR: u32 = 0x0020_60A0;
/// 焦点窗口的标题栏颜色。
#[cfg(target_os = "none")]
const TITLE_FOCUS: u32 = 0x0040_A0E0;
/// 标题文字颜色。
#[cfg(target_os = "none")]
const TITLE_TEXT: u32 = 0x00E0_F0FF;

/// 已创建的窗口；槽位顺序即 z 序（越靠后越靠上）。
#[cfg(target_os = "none")]
#[derive(Clone, Copy)]
struct Window {
    id: u64,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    virt: u64,
    handle: u64,
    client: u64,
    title: [u8; TITLE_MAX],
    title_length: usize,
}

#[cfg(target_os = "none")]
struct Screen {
    base: *mut u32,
    width: usize,
    height: usize,
    stride: usize,
}

#[cfg(target_os = "none")]
impl Screen {
    fn put(&self, x: usize, y: usize, color: u32) {
        if x >= self.width || y >= self.height {
            return;
        }
        // SAFETY: 坐标已裁剪到帧缓冲范围内。
        unsafe {
            self.base.add(y * self.stride + x).write_volatile(color);
        }
    }

    fn get(&self, x: usize, y: usize) -> u32 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        // SAFETY: 坐标已裁剪到帧缓冲范围内。
        unsafe { self.base.add(y * self.stride + x).read_volatile() }
    }
}

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
    let ratio = (row * 255 / height.max(1)) as u32;
    blend(DESKTOP_TOP, DESKTOP_BOTTOM, ratio)
}

/// 读取窗口表面中的一个像素。
#[cfg(target_os = "none")]
fn surface_pixel(window: &Window, x: usize, y: usize) -> u32 {
    // SAFETY: 表面是客户端转移过来的共享 Frame，长度已在创建时校验。
    unsafe {
        (window.virt as *const u32)
            .add(y * window.width + x)
            .read_volatile()
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
    let window_x = window.x;
    let window_y = window.y;

    let title_left = window_x.max(left);
    let title_right = (window_x + window.width).min(right);
    let title_top = window_y.max(top);
    let title_bottom = (window_y + TITLE_HEIGHT).min(bottom);
    if title_left < title_right && title_top < title_bottom {
        let color = if window.id == focus {
            TITLE_FOCUS
        } else {
            TITLE_COLOR
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
                        screen.put(pixel_x as usize, pixel_y as usize, TITLE_TEXT);
                    }
                });
            }
            pen_x += 8;
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

/// 重绘一个矩形区域：背景 + 自下而上的窗口。
#[cfg(target_os = "none")]
#[allow(clippy::too_many_arguments)]
fn compose_region(
    screen: &Screen,
    windows: &[Option<Window>; MAX_WINDOWS],
    focus: u64,
    font: &Font<'static>,
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
}

/// 重绘整个桌面。
#[cfg(target_os = "none")]
fn draw_desktop(screen: &Screen) {
    for row in 0..screen.height {
        let color = desktop_color(screen.height, row);
        for column in 0..screen.width {
            screen.put(column, row, color);
        }
    }
}

/// 把窗口提升到 z 序顶部。
#[cfg(target_os = "none")]
fn raise(windows: &mut [Option<Window>; MAX_WINDOWS], id: u64) {
    let Some(position) = windows
        .iter()
        .position(|window| window.as_ref().is_some_and(|window| window.id == id))
    else {
        return;
    };
    let Some(last) = windows.iter().rposition(Option::is_some) else {
        return;
    };
    if position < last {
        windows[position..=last].rotate_left(1);
    }
}

/// 重绘窗口（含标题栏）。
#[cfg(target_os = "none")]
fn damage_window(
    screen: &Screen,
    windows: &[Option<Window>; MAX_WINDOWS],
    focus: u64,
    font: &Font<'static>,
    window: &Window,
) {
    compose_region(
        screen,
        windows,
        focus,
        font,
        window.x,
        window.y,
        window.width,
        window.height + TITLE_HEIGHT,
    );
}

/// 重绘指定窗口的标题栏。
#[cfg(target_os = "none")]
fn damage_title(
    screen: &Screen,
    windows: &[Option<Window>; MAX_WINDOWS],
    focus: u64,
    font: &Font<'static>,
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
            window.x,
            window.y,
            window.width,
            TITLE_HEIGHT,
        );
    }
}

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
        damage_window(screen, windows, *focus, font, &window);
    }
    damage_title(screen, windows, *focus, font, previous);
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
    loop {
        // 输入来自内核控制台（自检阶段）；后续由 Input Driver 取代。
        while let Some(byte) = console_read() {
            if byte == TAB {
                cycle_focus(&screen, &mut windows, &mut focus, &font);
            } else {
                route_key(&windows, focus, byte);
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
                let y = screen
                    .height
                    .saturating_sub(height + TITLE_HEIGHT)
                    .saturating_div(2)
                    .saturating_add(offset)
                    .min(screen.height.saturating_sub(height + TITLE_HEIGHT));
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
                    damage_window(&screen, &windows, focus, &font, &window);
                }
                damage_title(&screen, &windows, focus, &font, previous);
                let _ = send(sender, DISPLAY_WINDOW_CREATED, id);
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
                    damage_title(&screen, &windows, focus, &font, id);
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
                damage_window(&screen, &windows, focus, &font, &window);
                damage_title(&screen, &windows, focus, &font, previous);
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

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(not(target_os = "none"))]
fn main() {
    eprintln!("hk-server-display 只在 x86_64-unknown-none 目标上构建");
}
