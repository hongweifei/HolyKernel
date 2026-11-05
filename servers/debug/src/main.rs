//! debug：最小调试窗口。
//!
//! 展示内核线程/对象/就绪线程/trace 计数、调度 tick、TSC 频率、本任务句柄数
//! 与服务在线状态；`debug-test` 模式下渲染一次后关闭 Display 退出。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use hk_server_api::display::{
    DISPLAY_CLICK, DISPLAY_CLOSE, DISPLAY_COMMIT, DISPLAY_CREATE_WINDOW, DISPLAY_FRAME_DONE,
    DISPLAY_KEY, DISPLAY_SHUTDOWN, DISPLAY_WINDOW_CREATED, SURFACE_VIRT, pack_size, set_title,
    unpack_key,
};
#[cfg(target_os = "none")]
use hk_server_api::{
    exit, frame_alloc, handle_count, kernel_stats, recv, send, send_handle, service_endpoint,
    ticks, tsc_frequency, write, write_u64,
};
#[cfg(target_os = "none")]
use lovyangfx_fonts::Font;
#[cfg(target_os = "none")]
use lovyangfx_fonts::fonts::efont_cn_16;

/// 窗口宽度。
#[cfg(target_os = "none")]
const WIDTH: usize = 560;
/// 窗口高度。
#[cfg(target_os = "none")]
const HEIGHT: usize = 400;
/// 行高。
#[cfg(target_os = "none")]
const LINE_HEIGHT: usize = 24;
/// 页面留白。
#[cfg(target_os = "none")]
const MARGIN: usize = 24;

/// 背景色。
#[cfg(target_os = "none")]
const BACKGROUND: u32 = 0x0018_2028;
/// 正文颜色。
#[cfg(target_os = "none")]
const FOREGROUND: u32 = 0x00C0_E0D0;
/// 标题颜色。
#[cfg(target_os = "none")]
const HEADER: u32 = 0x0080_C0FF;

/// 绘制一个字符。
#[cfg(target_os = "none")]
fn put_char(pixels: *mut u32, font: &Font<'static>, x: i32, y: i32, character: char, color: u32) {
    let Some(glyph) = font.glyph(character) else {
        return;
    };
    glyph.for_each_pixel(|px, py| {
        let screen_x = x + px;
        let screen_y = y + py;
        if screen_x >= 0
            && screen_y >= 0
            && (screen_x as usize) < WIDTH
            && (screen_y as usize) < HEIGHT
        {
            // SAFETY: 坐标已裁剪。
            unsafe {
                pixels
                    .add(screen_y as usize * WIDTH + screen_x as usize)
                    .write_volatile(color);
            }
        }
    });
}

/// 绘制一行文本。
#[cfg(target_os = "none")]
fn put_text(pixels: *mut u32, font: &Font<'static>, x: &mut i32, y: i32, text: &str, color: u32) {
    for character in text.chars() {
        put_char(pixels, font, *x, y, character, color);
        *x += if u32::from(character) > 0x7F { 16 } else { 8 };
    }
}

/// 绘制十进制数。
#[cfg(target_os = "none")]
fn put_number(
    pixels: *mut u32,
    font: &Font<'static>,
    x: &mut i32,
    y: i32,
    mut value: u64,
    color: u32,
) {
    let mut digits = [0_u8; 20];
    let mut length = 0;
    if value == 0 {
        digits[0] = b'0';
        length = 1;
    } else {
        while value > 0 {
            digits[length] = b'0' + (value % 10) as u8;
            value /= 10;
            length += 1;
        }
    }
    for index in (0..length).rev() {
        put_char(pixels, font, *x, y, digits[index] as char, color);
        *x += 8;
    }
}

/// 绘制一行“标签 + 数值”。
#[cfg(target_os = "none")]
fn put_row(pixels: *mut u32, font: &Font<'static>, y: i32, label: &str, value: u64, color: u32) {
    let mut x = MARGIN as i32;
    put_text(pixels, font, &mut x, y, label, color);
    put_number(pixels, font, &mut x, y, value, color);
}

/// 重绘整个窗口。
#[cfg(target_os = "none")]
fn render(pixels: *mut u32, font: &Font<'static>) {
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            // SAFETY: 共享表面已按 WIDTH×HEIGHT 映射。
            unsafe {
                pixels.add(y * WIDTH + x).write_volatile(BACKGROUND);
            }
        }
    }
    let stats = kernel_stats();
    let mut y = MARGIN as i32;
    let mut x = MARGIN as i32;
    put_text(pixels, font, &mut x, y, "HolyKernel Debug", HEADER);
    y += LINE_HEIGHT as i32 + 8;
    put_row(
        pixels,
        font,
        y,
        "线程: ",
        u64::from(stats.threads),
        FOREGROUND,
    );
    y += LINE_HEIGHT as i32;
    put_row(
        pixels,
        font,
        y,
        "对象: ",
        u64::from(stats.objects),
        FOREGROUND,
    );
    y += LINE_HEIGHT as i32;
    put_row(
        pixels,
        font,
        y,
        "就绪: ",
        u64::from(stats.runnable),
        FOREGROUND,
    );
    y += LINE_HEIGHT as i32;
    put_row(
        pixels,
        font,
        y,
        "Trace: ",
        u64::from(stats.trace_events),
        FOREGROUND,
    );
    y += LINE_HEIGHT as i32;
    put_row(pixels, font, y, "调度 tick: ", ticks(), FOREGROUND);
    y += LINE_HEIGHT as i32;
    put_row(pixels, font, y, "TSC Hz: ", tsc_frequency(), FOREGROUND);
    y += LINE_HEIGHT as i32;
    put_row(pixels, font, y, "本任务句柄: ", handle_count(), FOREGROUND);
    y += LINE_HEIGHT as i32;
    let display = service_endpoint("hk-server-display");
    let mut x = MARGIN as i32;
    put_text(
        pixels,
        font,
        &mut x,
        y,
        if display == 0 {
            "Display: 离线"
        } else {
            "Display: 在线"
        },
        FOREGROUND,
    );
}

/// 提交并等待显示服务完成合成。
#[cfg(target_os = "none")]
fn commit(endpoint: u64, display: u64, window: u64) {
    let _ = send(display, DISPLAY_COMMIT, window);
    loop {
        let (label, word, _sender) = recv(endpoint);
        if label == DISPLAY_FRAME_DONE && word == window {
            return;
        }
    }
}

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let display = hk_server_api::runtime::wait_service("hk-server-display").unwrap_or(0);
    if display == 0 {
        write(b"hk-server-debug: no display\n");
        exit(1)
    }
    let bytes = WIDTH * HEIGHT * 4;
    let handle = frame_alloc(bytes, SURFACE_VIRT);
    if handle == 0 || handle == u64::MAX {
        write(b"hk-server-debug: frame alloc failed\n");
        exit(2)
    }
    if send_handle(
        display,
        DISPLAY_CREATE_WINDOW,
        pack_size(WIDTH as u32, HEIGHT as u32),
        handle,
    ) == u64::MAX
    {
        write(b"hk-server-debug: create failed\n");
        exit(3)
    }
    let (label, window, _sender) = recv(endpoint);
    if label != DISPLAY_WINDOW_CREATED || window == 0 {
        write(b"hk-server-debug: window rejected\n");
        exit(4)
    }
    set_title(display, window, "Debug");

    let font = efont_cn_16();
    let pixels = SURFACE_VIRT as *mut u32;
    render(pixels, &font);
    commit(endpoint, display, window);
    let stats = kernel_stats();
    write(b"hk-server-debug: ready ");
    write_u64(window);
    write(b"\n");
    write(b"hk-server-debug: threads ");
    write_u64(u64::from(stats.threads));
    write(b"\n");
    write(b"hk-server-debug: objects ");
    write_u64(u64::from(stats.objects));
    write(b"\n");

    let mut cmdline_buffer = [0_u8; 128];
    let test = hk_server_api::boot_cmdline(&mut cmdline_buffer)
        .is_some_and(|line| line.contains("debug-test"));
    if test {
        write(b"hk-server-debug: done\n");
        let _ = send(display, DISPLAY_SHUTDOWN, 0);
        exit(0)
    }

    loop {
        let (label, word, _sender) = recv(endpoint);
        match label {
            DISPLAY_KEY => {
                let (_, byte) = unpack_key(word);
                match byte {
                    b'r' => {
                        render(pixels, &font);
                        commit(endpoint, display, window);
                    }
                    b'q' => {
                        let _ = send(display, DISPLAY_SHUTDOWN, 0);
                        exit(0)
                    }
                    _ => {}
                }
            }
            DISPLAY_CLICK => {}
            DISPLAY_CLOSE => exit(0),
            _ => {}
        }
    }
}

hk_server_api::server_boilerplate!();
