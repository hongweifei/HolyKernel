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
    DISPLAY_COMMIT, DISPLAY_CREATE_WINDOW, DISPLAY_FRAME_DONE, DISPLAY_KEY, DISPLAY_SHUTDOWN,
    DISPLAY_WINDOW_CREATED, pack_key, unpack_size,
};
#[cfg(target_os = "none")]
use hk_server_api::framebuffer;
#[cfg(target_os = "none")]
use hk_server_api::{
    close_handle, console_read, exit, frame_info, frame_map, recv_data_handle_try, send, write,
    write_u64, yield_now,
};

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

/// 重绘一个矩形区域：背景 + 自顶向下的窗口。
#[cfg(target_os = "none")]
fn compose_region(
    screen: &Screen,
    windows: &[Option<Window>; MAX_WINDOWS],
    focus: u64,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
) {
    let left = x.min(screen.width);
    let top = y.min(screen.height);
    let right = x.saturating_add(width).min(screen.width);
    let bottom = y.saturating_add(height).min(screen.height);
    for row in top..bottom {
        for column in left..right {
            let mut color = desktop_color(screen.height, row);
            for window in windows.iter().rev().flatten() {
                if column < window.x
                    || column >= window.x + window.width
                    || row < window.y
                    || row >= window.y + window.height + TITLE_HEIGHT
                {
                    continue;
                }
                if row < window.y + TITLE_HEIGHT {
                    color = if window.id == focus {
                        TITLE_FOCUS
                    } else {
                        TITLE_COLOR
                    };
                } else {
                    color =
                        surface_pixel(window, column - window.x, row - (window.y + TITLE_HEIGHT));
                }
                break;
            }
            screen.put(column, row, color);
        }
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
    window: &Window,
) {
    compose_region(
        screen,
        windows,
        focus,
        window.x,
        window.y,
        window.width,
        window.height + TITLE_HEIGHT,
    );
}

/// 焦点切换后重绘旧焦点窗口的标题栏。
#[cfg(target_os = "none")]
fn damage_title(screen: &Screen, windows: &[Option<Window>; MAX_WINDOWS], focus: u64, id: u64) {
    if id == 0 || id == focus {
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
fn cycle_focus(screen: &Screen, windows: &mut [Option<Window>; MAX_WINDOWS], focus: &mut u64) {
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
        damage_window(screen, windows, *focus, &window);
    }
    damage_title(screen, windows, *focus, previous);
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
                cycle_focus(&screen, &mut windows, &mut focus);
            } else {
                route_key(&windows, focus, byte);
            }
        }
        let mut payload = [0_u8; 8];
        let (label, word, sender, _length, handle) =
            recv_data_handle_try(endpoint, &mut payload, 0);
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
                let Some((_, length)) = frame_info(handle) else {
                    let _ = close_handle(handle);
                    let _ = send(sender, DISPLAY_WINDOW_CREATED, 0);
                    continue;
                };
                if length < bytes {
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
                });
                next_id += 1;
                focus = id;
                if let Some(window) = windows[slot] {
                    damage_window(&screen, &windows, focus, &window);
                }
                damage_title(&screen, &windows, focus, previous);
                let _ = send(sender, DISPLAY_WINDOW_CREATED, id);
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
                damage_window(&screen, &windows, focus, &window);
                damage_title(&screen, &windows, focus, previous);
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
