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
use hk_canvas::Surface;
#[cfg(target_os = "none")]
use hk_font::Font;
#[cfg(target_os = "none")]
use hk_server_api::display::{
    DISPLAY_CLICK, DISPLAY_CLOSE, DISPLAY_KEY, DISPLAY_RESIZE, DISPLAY_SHUTDOWN, SURFACE_COMMIT,
    SURFACE_CREATE, SURFACE_CREATED, SURFACE_FRAME_DONE, WINDOW_CREATE, WINDOW_CREATED,
    WINDOW_RESIZE, pack_size, pack_window_size, set_title, unpack_key, unpack_window_size,
};
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::input::{Decoder, Key};
#[cfg(target_os = "none")]
use hk_server_api::{
    CAPABILITY_SURFACE, close_handle, exit, fault_subscribe, kernel_stats, map_new, recv,
    recv_data, recv_data_try, region_unmap, send, send_handle, write, write_u64,
};
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
fn commit(endpoint: u64, display: u64, surface: u64) {
    let _ = send(display, SURFACE_COMMIT, surface);
    let mut buffer = [0_u8; 2048];
    loop {
        let (label, word, _sender, length) = recv_data(endpoint, &mut buffer);
        if label == hk_server_api::FAULT_EVENT {
            hk_debug::ingest(label, &buffer[..length]);
            continue;
        }
        if label == SURFACE_FRAME_DONE && word == surface {
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
    width: usize,
    height: usize,
    font: &Font,
    pixels: *mut u32,
    endpoint: u64,
    display: u64,
    surface: u64,
    window: u64,
) {
    let title = format!("Debug: {}", page.title());
    set_title(display, window, &title);
    // SAFETY: 表面帧已映射，长度为 width * height。
    let frame = unsafe { Surface::from_raw(pixels, width, height, width) };
    render(text, page.title(), scroll, width, height, font, &frame);
    commit(endpoint, display, surface);
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
    let mut width = WIDTH;
    let mut height = HEIGHT;
    let bytes = width * height * 4;
    let Some((handle, mut surface_region, surface_virt)) = map_new(CAPABILITY_SURFACE, bytes)
    else {
        write(b"hk-server-debug: frame alloc failed\n");
        exit(2)
    };
    if send_handle(
        display,
        SURFACE_CREATE,
        pack_size(width as u32, height as u32),
        handle,
    ) == u64::MAX
    {
        write(b"hk-server-debug: create failed\n");
        exit(3)
    }
    let (label, surface, _sender) = recv(endpoint);
    if label != SURFACE_CREATED || surface == 0 {
        write(b"hk-server-debug: surface rejected\n");
        exit(4)
    }
    let _ = send(display, WINDOW_CREATE, surface);
    let (label, window, _sender) = recv(endpoint);
    if label != WINDOW_CREATED || window == 0 {
        write(b"hk-server-debug: window rejected\n");
        exit(4)
    }

    let font = hk_canvas::default_font();
    write(b"hk-server-debug: font ");
    write(if font.has_scalable() {
        b"scalable ".as_slice()
    } else {
        b"bitmap ".as_slice()
    });
    write_u64(u64::from(font.size()));
    write(b"\n");
    let mut pixels = surface_virt as *mut u32;
    let mut page = Page::Overview;
    let mut text = page.fetch();
    let mut scroll = 0;
    show(
        page, &text, scroll, width, height, font, pixels, endpoint, display, surface, window,
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
    let _ = fault_subscribe(endpoint);
    if test {
        let base =
            u64::from_str_radix(env!("HK_IMAGE_BASE").trim_start_matches("0x"), 16).unwrap_or(0);
        for page in Page::ALL {
            let text = page.fetch();
            show(
                page, &text, 0, width, height, font, pixels, endpoint, display, surface, window,
            );
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
            let mut buffer = [0_u8; 2048];
            for _ in 0..100_000 {
                let (label, _word, _sender, length) = recv_data_try(endpoint, &mut buffer);
                if label == hk_server_api::FAULT_EVENT {
                    hk_debug::ingest(label, &buffer[..length]);
                }
                fault = Page::Fault.fetch();
                if fault.contains("异常：") {
                    break;
                }
                let _ = hk_server_api::yield_now();
            }
            show(
                Page::Fault,
                &fault,
                0,
                width,
                height,
                font,
                pixels,
                endpoint,
                display,
                surface,
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
        if let Some((name, offset)) = hk_debug::lookup("hk-server-debug", base) {
            write(b"hk-server-debug: service symbol ");
            write(format!("{base:#018x} {name}+0x{offset:x}").as_bytes());
            write(b"\n");
        }
        write(b"hk-server-debug: done\n");
        let _ = send(display, DISPLAY_SHUTDOWN, 0);
        exit(0)
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
                if new_width != width || new_height != height {
                    let bytes = new_width * new_height * 4;
                    let Some((handle, region, virt)) = map_new(CAPABILITY_SURFACE, bytes) else {
                        write(b"hk-server-debug: resize frame failed\n");
                        continue;
                    };
                    if send_handle(
                        display,
                        WINDOW_RESIZE,
                        pack_window_size(window, new_width as u32, new_height as u32),
                        handle,
                    ) == u64::MAX
                    {
                        let _ = close_handle(handle);
                        write(b"hk-server-debug: resize rejected\n");
                        continue;
                    }
                    let old_region = surface_region;
                    width = new_width;
                    height = new_height;
                    surface_region = region;
                    pixels = virt as *mut u32;
                    let _ = region_unmap(old_region);
                    let max = line_count(&text, width, font).saturating_sub(visible_lines(height));
                    scroll = scroll.min(max);
                    show(
                        page, &text, scroll, width, height, font, pixels, endpoint, display,
                        surface, window,
                    );
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
                    show(
                        page, &text, scroll, width, height, font, pixels, endpoint, display,
                        surface, window,
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
