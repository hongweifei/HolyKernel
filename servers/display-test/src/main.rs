//! display-test：创建两个共享表面窗口，提交合成、切换焦点并校验按键路由。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;
#[cfg(target_os = "none")]
use hk_server_api::display::{
    DISPLAY_COMMIT, DISPLAY_CREATE_WINDOW, DISPLAY_FRAME_DONE, DISPLAY_KEY, DISPLAY_SHUTDOWN,
    DISPLAY_WINDOW_CREATED, SURFACE_VIRT, pack_size, unpack_key,
};
#[cfg(target_os = "none")]
use hk_server_api::{
    exit, frame_alloc, recv, send, send_handle, service_endpoint, write, write_u64, yield_now,
};

/// 两个窗口表面的地址间隔。
#[cfg(target_os = "none")]
const SURFACE_STRIDE: u64 = 0x10_0000;
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

    let first = create_window(display, endpoint, 320, 200, 0, 0x0033_99FF, 0x0008_1018);
    hk_server_api::display::set_title(display, first, "Window 1");
    let second = create_window(
        display,
        endpoint,
        280,
        180,
        SURFACE_STRIDE,
        0x00FF_9933,
        0x0020_1008,
    );
    hk_server_api::display::set_title(display, second, "Window 2");

    let _ = send(display, DISPLAY_COMMIT, first);
    let _ = send(display, DISPLAY_COMMIT, second);
    for _ in 0..2 {
        let (label, _word, _sender) = recv(endpoint);
        if label != DISPLAY_FRAME_DONE {
            write(b"hk-server-display-test: no frame\n");
            exit(5)
        }
    }

    write(b"hk-server-display-test: ready\n");
    let (label, word, _sender) = recv(endpoint);
    if label != DISPLAY_KEY {
        write(b"hk-server-display-test: no key\n");
        exit(6)
    }
    let (window, byte) = unpack_key(word);
    write(b"hk-server-display-test: key ");
    write_u64(u64::from(byte));
    write(b" win ");
    write_u64(window);
    write(b"\n");

    write(b"hk-server-display-test: done\n");
    let _ = send(display, DISPLAY_SHUTDOWN, 0);
    exit(0)
}

/// 分配表面、绘制棋盘并创建窗口，返回窗口编号。
#[cfg(target_os = "none")]
fn create_window(
    display: u64,
    endpoint: u64,
    width: u32,
    height: u32,
    offset: u64,
    accent: u32,
    shade: u32,
) -> u64 {
    let bytes = width as usize * height as usize * 4;
    let virt = SURFACE_VIRT + offset;
    let handle = frame_alloc(bytes, virt);
    if handle == 0 || handle == u64::MAX {
        write(b"hk-server-display-test: frame alloc failed\n");
        exit(2)
    }

    let pixels = virt as *mut u32;
    for y in 0..height {
        for x in 0..width {
            let color = if x < 2 || y < 2 || x + 2 >= width || y + 2 >= height {
                0x00FF_FFFF
            } else if ((x / 16 + y / 16) % 2) == 0 {
                accent
            } else {
                shade
            };
            // SAFETY: 共享 Frame 已按宽高分配并映射到该虚拟地址。
            unsafe {
                pixels
                    .add(y as usize * width as usize + x as usize)
                    .write_volatile(color);
            }
        }
    }

    if send_handle(
        display,
        DISPLAY_CREATE_WINDOW,
        pack_size(width, height),
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
    window
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
