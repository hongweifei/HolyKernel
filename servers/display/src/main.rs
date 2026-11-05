//! display：帧缓冲合成器与窗口服务。
//!
//! 只有本服务持有帧缓冲授权。客户端把共享 Frame 作为窗口表面，经句柄转移
//! 交给本服务；合成时直接把表面像素写入帧缓冲，不复制整块表面。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;
#[cfg(target_os = "none")]
use hk_server_api::display::{
    DISPLAY_COMMIT, DISPLAY_CREATE_WINDOW, DISPLAY_FRAME_DONE, DISPLAY_SHUTDOWN,
    DISPLAY_WINDOW_CREATED, unpack_size,
};
#[cfg(target_os = "none")]
use hk_server_api::framebuffer;
#[cfg(target_os = "none")]
use hk_server_api::{
    close_handle, exit, frame_info, frame_map, recv_data_handle, send, write, write_u64,
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

/// 桌面背景顶部颜色。
#[cfg(target_os = "none")]
const DESKTOP_TOP: u32 = 0x0010_1820;
/// 桌面背景底部颜色。
#[cfg(target_os = "none")]
const DESKTOP_BOTTOM: u32 = 0x0030_4860;
/// 标题栏颜色。
#[cfg(target_os = "none")]
const TITLE_COLOR: u32 = 0x0020_60A0;

/// 已创建的窗口。
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

    fn fill(&self, x: usize, y: usize, width: usize, height: usize, color: u32) {
        for row in y..(y + height).min(self.height) {
            for column in x..(x + width).min(self.width) {
                // SAFETY: 坐标已裁剪到帧缓冲范围内。
                unsafe {
                    self.base
                        .add(row * self.stride + column)
                        .write_volatile(color);
                }
            }
        }
    }
}

/// 绘制桌面渐变背景。
#[cfg(target_os = "none")]
fn draw_desktop(screen: &Screen) {
    for y in 0..screen.height {
        let ratio = (y * 255 / screen.height.max(1)) as u32;
        let color = blend(DESKTOP_TOP, DESKTOP_BOTTOM, ratio);
        for x in 0..screen.width {
            screen.put(x, y, color);
        }
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

/// 绘制一个窗口：标题栏 + 客户端表面。
#[cfg(target_os = "none")]
fn draw_window(screen: &Screen, window: &Window) {
    screen.fill(window.x, window.y, window.width, TITLE_HEIGHT, TITLE_COLOR);
    let surface = window.virt as *const u32;
    for row in 0..window.height {
        let target_y = window.y + TITLE_HEIGHT + row;
        for column in 0..window.width {
            // SAFETY: 表面是客户端转移过来的共享 Frame，长度已在创建时校验。
            let color = unsafe { surface.add(row * window.width + column).read_volatile() };
            screen.put(window.x + column, target_y, color);
        }
    }
}

/// 合成桌面与全部窗口。
#[cfg(target_os = "none")]
fn compose(screen: &Screen, windows: &[Option<Window>; MAX_WINDOWS]) {
    draw_desktop(screen);
    for window in windows.iter().flatten() {
        draw_window(screen, window);
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
    loop {
        let mut payload = [0_u8; 8];
        let (label, word, sender, _length, handle) = recv_data_handle(endpoint, &mut payload, 0);
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
                windows[slot] = Some(Window {
                    id,
                    x,
                    y,
                    width,
                    height,
                    virt,
                    handle,
                });
                next_id += 1;
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
                compose(&screen, &windows);
                let surface = window.virt as *const u32;
                // SAFETY: 表面长度已在创建时校验。
                let expected = unsafe { surface.read_volatile() };
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
