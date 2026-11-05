//! debug：调试窗口服务。
//!
//! 分页展示内核概览、任务、服务、trace、运行时统计与最近一次用户异常回溯；
//! `debug-test` 模式下依次渲染各页后关闭 Display 退出。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::format;

#[cfg(target_os = "none")]
mod pages;
#[cfg(target_os = "none")]
mod render;

#[cfg(target_os = "none")]
use hk_font::Font;
#[cfg(target_os = "none")]
use hk_server_api::display::{
    DISPLAY_CLICK, DISPLAY_CLOSE, DISPLAY_COMMIT, DISPLAY_CREATE_WINDOW, DISPLAY_FRAME_DONE,
    DISPLAY_KEY, DISPLAY_SHUTDOWN, DISPLAY_WINDOW_CREATED, SURFACE_VIRT, pack_size, set_title,
    unpack_key,
};
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::input::{Decoder, Key};
#[cfg(target_os = "none")]
use hk_server_api::{exit, frame_alloc, kernel_stats, recv, send, send_handle, write, write_u64};
#[cfg(target_os = "none")]
#[cfg(target_os = "none")]
use pages::Page;
#[cfg(target_os = "none")]
use render::{HEIGHT, WIDTH, line_count, render, visible_lines};

/// 服务堆大小。
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 512 * 1024;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

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

#[cfg(target_os = "none")]
#[allow(clippy::too_many_arguments)]
fn show(
    page: Page,
    text: &str,
    scroll: usize,
    font: &Font,
    pixels: *mut u32,
    endpoint: u64,
    display: u64,
    window: u64,
) {
    let title = format!("Debug: {}", page.title());
    set_title(display, window, &title);
    render(text, page.title(), scroll, font, pixels);
    commit(endpoint, display, window);
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

    let font = Font::new(16);
    write(b"hk-server-debug: font ");
    write(if font.has_scalable() {
        b"scalable ".as_slice()
    } else {
        b"bitmap ".as_slice()
    });
    write_u64(u64::from(font.size()));
    write(b"\n");
    let pixels = SURFACE_VIRT as *mut u32;
    let mut page = Page::Overview;
    let mut text = page.fetch();
    let mut scroll = 0;
    show(
        page, &text, scroll, &font, pixels, endpoint, display, window,
    );

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
    let cmdline = hk_server_api::boot_cmdline(&mut cmdline_buffer).unwrap_or("");
    let fault_test = cmdline.contains("debug-crash");
    let test = fault_test || cmdline.contains("debug-test");
    if test {
        for page in Page::ALL {
            let text = page.fetch();
            show(page, &text, 0, &font, pixels, endpoint, display, window);
            write(b"hk-server-debug: view ");
            write(page.slug().as_bytes());
            write(b"\n");
        }
        if fault_test {
            let child = hk_server_api::fork();
            if child == 0 {
                // SAFETY: 自检子任务故意触发非法指令，验证异常记录与符号化。
                unsafe {
                    core::arch::asm!("ud2", options(noreturn));
                }
            }
            let mut fault = Page::Fault.fetch();
            for _ in 0..100_000 {
                if fault.contains("异常：") {
                    break;
                }
                let _ = hk_server_api::yield_now();
                fault = Page::Fault.fetch();
            }
            show(
                Page::Fault,
                &fault,
                0,
                &font,
                pixels,
                endpoint,
                display,
                window,
            );
            if fault.contains("[hk-server-debug]") {
                write(b"hk-server-debug: fault symbolized\n");
            } else {
                write(b"hk-server-debug: fault raw\n");
            }
        }
        let mut symbol = [0_u8; 256];
        if let Ok(length) = hk_server_api::root::call(
            hk_server_api::root::ROOT_OP_SYMBOLIZE,
            "0xffffffff80000000",
            &mut symbol,
        ) {
            write(b"hk-server-debug: symbol ");
            write(
                core::str::from_utf8(&symbol[..length])
                    .unwrap_or("?")
                    .trim_end()
                    .as_bytes(),
            );
            write(b"\n");
        }
        write(b"hk-server-debug: done\n");
        let _ = send(display, DISPLAY_SHUTDOWN, 0);
        exit(0)
    }

    let mut keys = Decoder::new();
    loop {
        let (label, word, _sender) = recv(endpoint);
        match label {
            DISPLAY_KEY => {
                let (_, byte) = unpack_key(word);
                let Some(key) = keys.feed(byte) else {
                    continue;
                };
                let max = line_count(&text, &font).saturating_sub(visible_lines());
                let visible = visible_lines();
                let mut update = false;
                match key {
                    Key::Byte(byte @ b'1'..=b'6') => {
                        page = Page::ALL[usize::from(byte - b'1')];
                        text = page.fetch();
                        scroll = 0;
                        update = true;
                    }
                    Key::Byte(b'j') | Key::Down => {
                        scroll = scroll.saturating_add(1).min(max);
                        update = true;
                    }
                    Key::Byte(b'k') | Key::Up => {
                        scroll = scroll.saturating_sub(1);
                        update = true;
                    }
                    Key::Byte(b' ') | Key::PageDown => {
                        scroll = scroll.saturating_add(visible).min(max);
                        update = true;
                    }
                    Key::Byte(b'b') | Key::PageUp => {
                        scroll = scroll.saturating_sub(visible);
                        update = true;
                    }
                    Key::Byte(b'u') => {
                        scroll = scroll.saturating_sub(10);
                        update = true;
                    }
                    Key::Byte(b'd') => {
                        scroll = scroll.saturating_add(10).min(max);
                        update = true;
                    }
                    Key::Byte(b'g') | Key::Home => {
                        scroll = 0;
                        update = true;
                    }
                    Key::Byte(b'G') | Key::End => {
                        scroll = max;
                        update = true;
                    }
                    Key::Byte(b'r') => {
                        text = page.fetch();
                        let max = line_count(&text, &font).saturating_sub(visible);
                        scroll = scroll.min(max);
                        update = true;
                    }
                    Key::Byte(b'q') => {
                        let _ = send(display, DISPLAY_SHUTDOWN, 0);
                        exit(0)
                    }
                    _ => {}
                }
                if update {
                    show(
                        page, &text, scroll, &font, pixels, endpoint, display, window,
                    );
                }
            }
            DISPLAY_CLICK => {}
            DISPLAY_CLOSE => exit(0),
            _ => {}
        }
    }
}

hk_server_api::server_boilerplate!();
