//! pointer-test：创建窗口，等待鼠标点击焦点事件。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use hk_server_api::display::{
    DISPLAY_CLICK, DISPLAY_COMMIT, DISPLAY_CREATE_WINDOW, DISPLAY_FRAME_DONE, DISPLAY_SHUTDOWN,
    DISPLAY_WINDOW_CREATED, SURFACE_VIRT, pack_size, set_title,
};
#[cfg(target_os = "none")]
use hk_server_api::{exit, frame_alloc, recv, send, send_handle, write, write_u64};

/// 窗口宽度。
#[cfg(target_os = "none")]
const WIDTH: u32 = 400;
/// 窗口高度。
#[cfg(target_os = "none")]
const HEIGHT: u32 = 300;
/// 等待显示服务出现的重试次数。
#[cfg(target_os = "none")]

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let display = wait_display();
    if display == 0 {
        write(b"hk-server-pointer-test: display not found\n");
        exit(1)
    }

    let bytes = WIDTH as usize * HEIGHT as usize * 4;
    let handle = frame_alloc(bytes, SURFACE_VIRT);
    if handle == 0 || handle == u64::MAX {
        write(b"hk-server-pointer-test: frame alloc failed\n");
        exit(2)
    }
    let pixels = SURFACE_VIRT as *mut u32;
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let color = if x < 3 || y < 3 || x + 3 >= WIDTH || y + 3 >= HEIGHT {
                0x00FF_FFFF
            } else if ((x / 24 + y / 24) % 2) == 0 {
                0x0040_C080
            } else {
                0x0010_2010
            };
            // SAFETY: 共享 Frame 已按宽高分配并映射到 SURFACE_VIRT。
            unsafe {
                pixels
                    .add(y as usize * WIDTH as usize + x as usize)
                    .write_volatile(color);
            }
        }
    }

    if send_handle(
        display,
        DISPLAY_CREATE_WINDOW,
        pack_size(WIDTH, HEIGHT),
        handle,
    ) == u64::MAX
    {
        write(b"hk-server-pointer-test: create failed\n");
        exit(3)
    }
    let (label, window, _sender) = recv(endpoint);
    if label != DISPLAY_WINDOW_CREATED || window == 0 {
        write(b"hk-server-pointer-test: window rejected\n");
        exit(4)
    }
    set_title(display, window, "Pointer");
    let _ = send(display, DISPLAY_COMMIT, window);
    let (label, _word, _sender) = recv(endpoint);
    if label != DISPLAY_FRAME_DONE {
        write(b"hk-server-pointer-test: no frame\n");
        exit(5)
    }

    write(b"hk-server-pointer-test: ready\n");
    let (label, word, _sender) = recv(endpoint);
    if label != DISPLAY_CLICK || word != window {
        write(b"hk-server-pointer-test: no click\n");
        exit(6)
    }
    write(b"hk-server-pointer-test: clicked ");
    write_u64(word);
    write(b"\n");
    let _ = send(display, DISPLAY_SHUTDOWN, 0);
    exit(0)
}

#[cfg(target_os = "none")]
fn wait_display() -> u64 {
    hk_server_api::runtime::wait_service(hk_server_api::DISPLAY_SERVICE).unwrap_or(0)
}

hk_server_api::server_boilerplate!();
