//! pointer-test：创建窗口，等待鼠标点击焦点事件。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use hk_server_api::display::{
    DISPLAY_CLICK, DISPLAY_CLOSE, DISPLAY_RESIZE, DISPLAY_SHUTDOWN, SURFACE_COMMIT, SURFACE_CREATE,
    SURFACE_CREATED, SURFACE_FRAME_DONE, SURFACE_VIRT, WINDOW_CREATE, WINDOW_CREATED,
    WINDOW_RESIZE, pack_size, pack_window_size, set_title, unpack_window_size,
};
#[cfg(target_os = "none")]
use hk_server_api::{exit, frame_alloc, frame_unmap, recv, send, send_handle, write, write_u64};

/// 窗口宽度。
#[cfg(target_os = "none")]
const WIDTH: u32 = 400;
/// 窗口高度。
#[cfg(target_os = "none")]
const HEIGHT: u32 = 300;

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let display = wait_display();
    if display == 0 {
        write(b"hk-server-pointer-test: display not found\n");
        exit(1)
    }

    let mut width = WIDTH;
    let mut height = HEIGHT;
    let bytes = width as usize * height as usize * 4;
    let handle = frame_alloc(bytes, SURFACE_VIRT);
    if handle == 0 || handle == u64::MAX {
        write(b"hk-server-pointer-test: frame alloc failed\n");
        exit(2)
    }
    let pixels = SURFACE_VIRT as *mut u32;
    paint(pixels, width, height);

    if send_handle(display, SURFACE_CREATE, pack_size(width, height), handle) == u64::MAX {
        write(b"hk-server-pointer-test: create failed\n");
        exit(3)
    }
    let (label, surface, _sender) = recv(endpoint);
    if label != SURFACE_CREATED || surface == 0 {
        write(b"hk-server-pointer-test: surface rejected\n");
        exit(4)
    }
    let _ = send(display, WINDOW_CREATE, surface);
    let (label, window, _sender) = recv(endpoint);
    if label != WINDOW_CREATED || window == 0 {
        write(b"hk-server-pointer-test: window rejected\n");
        exit(4)
    }
    set_title(display, window, "Pointer");
    let _ = send(display, SURFACE_COMMIT, surface);
    let (label, word, _sender) = recv(endpoint);
    if label != SURFACE_FRAME_DONE || word != surface {
        write(b"hk-server-pointer-test: no frame\n");
        exit(5)
    }

    write(b"hk-server-pointer-test: ready\n");
    loop {
        let (label, word, _sender) = recv(endpoint);
        match label {
            DISPLAY_RESIZE => {
                let (_, new_width, new_height) = unpack_window_size(word);
                if new_width != width || new_height != height {
                    let _ = frame_unmap(SURFACE_VIRT, width as usize * height as usize * 4);
                    let handle =
                        frame_alloc(new_width as usize * new_height as usize * 4, SURFACE_VIRT);
                    if handle == 0 || handle == u64::MAX {
                        write(b"hk-server-pointer-test: resize frame failed\n");
                        exit(7)
                    }
                    if send_handle(
                        display,
                        WINDOW_RESIZE,
                        pack_window_size(window, new_width, new_height),
                        handle,
                    ) == u64::MAX
                    {
                        write(b"hk-server-pointer-test: resize rejected\n");
                        exit(7)
                    }
                    width = new_width;
                    height = new_height;
                    paint(pixels, width, height);
                    let _ = send(display, SURFACE_COMMIT, surface);
                    write(b"hk-server-pointer-test: resize ");
                    write_u64(u64::from(width));
                    write(b"x");
                    write_u64(u64::from(height));
                    write(b"\n");
                }
            }
            DISPLAY_CLICK if word == window => break,
            DISPLAY_CLOSE => {
                write(b"hk-server-pointer-test: no click\n");
                exit(6)
            }
            _ => {}
        }
    }
    write(b"hk-server-pointer-test: clicked ");
    write_u64(window);
    write(b"\n");
    loop {
        let (label, word, _sender) = recv(endpoint);
        if label == DISPLAY_CLOSE && word == window {
            break;
        }
    }
    write(b"hk-server-pointer-test: closed ");
    write_u64(window);
    write(b"\n");
    let _ = send(display, DISPLAY_SHUTDOWN, 0);
    exit(0)
}

#[cfg(target_os = "none")]
fn wait_display() -> u64 {
    hk_server_api::runtime::wait_service(hk_server_api::DISPLAY_SERVICE).unwrap_or(0)
}

/// 按当前尺寸绘制测试图案。
#[cfg(target_os = "none")]
fn paint(pixels: *mut u32, width: u32, height: u32) {
    for y in 0..height {
        for x in 0..width {
            let color = if x < 3 || y < 3 || x + 3 >= width || y + 3 >= height {
                0x00FF_FFFF
            } else if ((x / 24 + y / 24) % 2) == 0 {
                0x0040_C080
            } else {
                0x0010_2010
            };
            // SAFETY: 共享 Frame 已按宽高分配并映射到 SURFACE_VIRT。
            unsafe {
                pixels
                    .add(y as usize * width as usize + x as usize)
                    .write_volatile(color);
            }
        }
    }
}

hk_server_api::server_boilerplate!();
