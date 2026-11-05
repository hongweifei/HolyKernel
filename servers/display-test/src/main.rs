//! display-test：创建共享表面窗口，提交给 Display 合成并验证。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;
#[cfg(target_os = "none")]
use hk_server_api::display::{
    DISPLAY_COMMIT, DISPLAY_CREATE_WINDOW, DISPLAY_FRAME_DONE, DISPLAY_SHUTDOWN,
    DISPLAY_WINDOW_CREATED, SURFACE_VIRT, pack_size,
};
#[cfg(target_os = "none")]
use hk_server_api::{
    exit, frame_alloc, recv, send, send_handle, service_endpoint, write, write_u64, yield_now,
};

/// 窗口宽度。
#[cfg(target_os = "none")]
const WIDTH: u32 = 320;
/// 窗口高度。
#[cfg(target_os = "none")]
const HEIGHT: u32 = 200;
/// 等待显示服务出现的重试次数。
#[cfg(target_os = "none")]
const WAIT_ATTEMPTS: usize = 4096;

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let display = wait_display();
    if display == 0 {
        write(b"hk-server-display-test: display not found\n");
        exit(1)
    }

    let bytes = WIDTH as usize * HEIGHT as usize * 4;
    let handle = frame_alloc(bytes, SURFACE_VIRT);
    if handle == 0 || handle == u64::MAX {
        write(b"hk-server-display-test: frame alloc failed\n");
        exit(2)
    }

    let pixels = SURFACE_VIRT as *mut u32;
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let color = if x < 2 || y < 2 || x + 2 >= WIDTH || y + 2 >= HEIGHT {
                0x00FF_FFFF
            } else if ((x / 16 + y / 16) % 2) == 0 {
                0x0033_99FF
            } else {
                0x0008_1018
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
        write(b"hk-server-display-test: create failed\n");
        exit(3)
    }

    let (label, window, _sender) = recv(endpoint);
    if label != DISPLAY_WINDOW_CREATED || window == 0 {
        write(b"hk-server-display-test: window rejected\n");
        exit(4)
    }
    write(b"hk-server-display-test: window ");
    write_u64(window);
    write(b"\n");

    let _ = send(display, DISPLAY_COMMIT, window);
    let (label, _word, _sender) = recv(endpoint);
    if label != DISPLAY_FRAME_DONE {
        write(b"hk-server-display-test: no frame\n");
        exit(5)
    }

    write(b"hk-server-display-test: done\n");
    let _ = send(display, DISPLAY_SHUTDOWN, 0);
    exit(0)
}

#[cfg(target_os = "none")]
fn wait_display() -> u64 {
    for _ in 0..WAIT_ATTEMPTS {
        let endpoint = service_endpoint(hk_server_api::DISPLAY_SERVICE);
        if endpoint != 0 {
            return endpoint;
        }
        let _ = yield_now();
    }
    0
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
    eprintln!("hk-server-display-test 只在 x86_64-unknown-none 目标上构建");
}
