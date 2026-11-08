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
mod selftest;

#[cfg(target_os = "none")]
use hk_canvas::Surface;
#[cfg(target_os = "none")]
use hk_font::Font;
#[cfg(target_os = "none")]
use hk_proto::display::{
    DISPLAY_CLICK, DISPLAY_CLOSE, DISPLAY_KEY, DISPLAY_RESIZE, DISPLAY_SHUTDOWN, Window,
    unpack_key, unpack_window_size,
};
#[cfg(target_os = "none")]
use hk_proto::input::{Decoder, Key};
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::{exit, fault_subscribe, kernel_stats, recv_data, send, write, write_u64};
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

#[cfg(target_os = "none")]
pub(crate) fn show(page: Page, text: &str, scroll: usize, font: &Font, window: &Window) {
    let title = format!("Debug: {}", page.title());
    window.set_title(&title);
    let (width, height) = (window.width(), window.height());
    // SAFETY: 表面帧已映射，长度为 width * height。
    let frame = unsafe { Surface::from_raw(window.pixels(), width, height, width) };
    render(text, page.title(), scroll, width, height, font, &frame);
    window.commit_with(|label, _, data| {
        if label == hk_server_api::FAULT_EVENT {
            hk_debug::ingest(label, data);
        }
    });
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
    let Some(mut window) = Window::attach(endpoint, display, WIDTH, HEIGHT) else {
        write(b"hk-server-debug: attach failed\n");
        exit(2)
    };
    let (mut width, mut height) = (window.width(), window.height());

    let font = hk_canvas::default_font();
    write(b"hk-server-debug: font ");
    write(if font.has_scalable() {
        b"scalable ".as_slice()
    } else {
        b"bitmap ".as_slice()
    });
    write_u64(u64::from(font.size()));
    write(b"\n");
    let mut page = Page::Overview;
    let mut text = page.fetch();
    let mut scroll = 0;
    show(page, &text, scroll, font, &window);

    let stats = kernel_stats();
    write(b"hk-server-debug: ready ");
    write_u64(window.window());
    write(b"\n");
    write(b"hk-server-debug: threads ");
    write_u64(u64::from(stats.threads));
    write(b"\n");
    write(b"hk-server-debug: objects ");
    write_u64(u64::from(stats.objects));
    write(b"\n");

    let mut cmdline_buffer = [0_u8; 128];
    let cmdline = hk_server_api::boot_cmdline(&mut cmdline_buffer).unwrap_or("");
    let _ = fault_subscribe(endpoint);
    if let Some(fault_test) = selftest::selected(cmdline) {
        selftest::run(endpoint, display, font, &window, fault_test);
    }

    let mut keys = Decoder::new();
    let mut buffer = [0_u8; 2048];
    loop {
        let (label, word, _sender, length) = recv_data(endpoint, &mut buffer);
        if label == hk_server_api::FAULT_EVENT {
            hk_debug::ingest(label, &buffer[..length]);
            continue;
        }
        match label {
            DISPLAY_RESIZE => {
                let (_, new_width, new_height) = unpack_window_size(word);
                let new_width = new_width as usize;
                let new_height = new_height as usize;
                if new_width == 0 || new_height == 0 {
                    continue;
                }
                if window.resize(new_width, new_height) {
                    width = new_width;
                    height = new_height;
                    let max = line_count(&text, width, font).saturating_sub(visible_lines(height));
                    scroll = scroll.min(max);
                    show(page, &text, scroll, font, &window);
                }
            }
            DISPLAY_KEY => {
                let (_, byte) = unpack_key(word);
                let Some(key) = keys.feed(byte) else {
                    continue;
                };
                let max = line_count(&text, width, font).saturating_sub(visible_lines(height));
                let visible = visible_lines(height);
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
                        let max = line_count(&text, width, font).saturating_sub(visible);
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
                    show(page, &text, scroll, font, &window);
                }
            }
            DISPLAY_CLICK => {}
            DISPLAY_CLOSE => exit(0),
            _ => {}
        }
    }
}

hk_server_api::server_boilerplate!();
