//! display-test：表面、窗口与 seat 协议自检。
//!
//! 流程：创建两个表面并挂成窗口 → 一个走局部 damage 提交，一个走整面提交 →
//! 设置焦点 → 剪贴板往返 → 等待按键路由。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use hk_proto::display::{
    DISPLAY_KEY, DISPLAY_SHUTDOWN, SEAT_CLIPBOARD, SEAT_FOCUS, SEAT_GET_CLIPBOARD,
    SEAT_SET_CLIPBOARD, SURFACE_COMMIT, SURFACE_CREATE, SURFACE_CREATED, SURFACE_DAMAGE,
    SURFACE_FRAME_DONE, WINDOW_CREATE, WINDOW_CREATED, pack_size, pack_surface_rect, unpack_key,
};
#[cfg(target_os = "none")]
use hk_server_api::{
    CapabilityKind, exit, map_new, recv, recv_data, send, send_data, send_handle, write, write_u64,
};

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let display = wait_display();
    if display == 0 {
        write(b"hk-server-display-test: display not found\n");
        exit(1)
    }

    let first_surface = create_surface(display, endpoint, 320, 200, 0x0033_99FF, 0x0008_1018);
    let first = create_window(display, endpoint, first_surface);
    hk_proto::display::set_title(display, first, "Window 1");
    let second_surface = create_surface(display, endpoint, 280, 180, 0x00FF_9933, 0x0020_1008);
    let second = create_window(display, endpoint, second_surface);
    hk_proto::display::set_title(display, second, "Window 2");

    // 第一个窗口只提交一块局部 damage，验证按区域重绘。
    let _ = send_data(
        display,
        SURFACE_DAMAGE,
        &pack_surface_rect(first_surface, 0, 0, 64, 64),
    );
    let _ = send(display, SURFACE_COMMIT, first_surface);
    wait_frame(endpoint, SURFACE_FRAME_DONE, first_surface, 5);
    let _ = send(display, SURFACE_COMMIT, second_surface);
    wait_frame(endpoint, SURFACE_FRAME_DONE, second_surface, 6);

    let _ = send(display, SEAT_FOCUS, second);

    if !clipboard_round_trip(display, endpoint) {
        exit(7)
    }

    write(b"hk-server-display-test: ready\n");
    let (label, word, _sender) = recv(endpoint);
    if label != DISPLAY_KEY {
        write(b"hk-server-display-test: no key\n");
        exit(8)
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

/// 分配表面、绘制棋盘并创建表面，返回表面编号。
#[cfg(target_os = "none")]
fn create_surface(
    display: u64,
    endpoint: u64,
    width: u32,
    height: u32,
    accent: u32,
    shade: u32,
) -> u64 {
    let bytes = width as usize * height as usize * 4;
    let Some((handle, _region, virt)) = map_new(CapabilityKind::Surface, bytes) else {
        write(b"hk-server-display-test: frame alloc failed\n");
        exit(2)
    };

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

    if send_handle(display, SURFACE_CREATE, pack_size(width, height), handle) == u64::MAX {
        write(b"hk-server-display-test: create failed\n");
        exit(3)
    }

    let (label, surface, _sender) = recv(endpoint);
    if label != SURFACE_CREATED || surface == 0 {
        write(b"hk-server-display-test: surface rejected\n");
        exit(4)
    }
    write(b"hk-server-display-test: surface ");
    write_u64(surface);
    write(b"\n");
    surface
}

/// 把表面挂成窗口，返回窗口编号。
#[cfg(target_os = "none")]
fn create_window(display: u64, endpoint: u64, surface: u64) -> u64 {
    let _ = send(display, WINDOW_CREATE, surface);
    let (label, window, _sender) = recv(endpoint);
    if label != WINDOW_CREATED || window == 0 {
        write(b"hk-server-display-test: window rejected\n");
        exit(5)
    }
    write(b"hk-server-display-test: window ");
    write_u64(window);
    write(b"\n");
    window
}

/// 等待指定表面的一帧完成。
#[cfg(target_os = "none")]
fn wait_frame(endpoint: u64, expected: u64, surface: u64, code: u64) {
    let (label, word, _sender) = recv(endpoint);
    if label != expected || word != surface {
        write(b"hk-server-display-test: no frame\n");
        exit(code)
    }
}

/// 写入剪贴板再读回，校验内容。
#[cfg(target_os = "none")]
fn clipboard_round_trip(display: u64, endpoint: u64) -> bool {
    const TEXT: &[u8] = b"holy";
    let _ = send_data(display, SEAT_SET_CLIPBOARD, TEXT);
    let _ = send(display, SEAT_GET_CLIPBOARD, 0);
    let mut buffer = [0_u8; 64];
    for _ in 0..8 {
        let (label, _word, _sender, length) = recv_data(endpoint, &mut buffer);
        if label != SEAT_CLIPBOARD {
            continue;
        }
        if length == TEXT.len() && buffer[..length] == *TEXT {
            write(b"hk-server-display-test: clipboard ");
            write(TEXT);
            write(b"\n");
            return true;
        }
        return false;
    }
    write(b"hk-server-display-test: clipboard timeout\n");
    false
}

#[cfg(target_os = "none")]
fn wait_display() -> u64 {
    hk_server_api::runtime::wait_service(hk_server_api::DISPLAY_SERVICE).unwrap_or(0)
}

hk_server_api::server_boilerplate!();
