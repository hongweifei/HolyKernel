//! holydoc：原生活文档窗口服务。
//!
//! 服务入口与事件循环；H: 卷加载在 `drive`，文档模型在 `document`，
//! 指令解析在 `markup`，绘制在 `render`，窗口提交在 `window`，自检在
//! `selftest`。整页绘制交给工作线程，主线程只负责事件、拷贝与提交。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
mod document;
#[cfg(target_os = "none")]
mod drive;
#[cfg(target_os = "none")]
mod exec;
#[cfg(target_os = "none")]
mod markup;
#[cfg(target_os = "none")]
mod render;
#[cfg(target_os = "none")]
mod selftest;
#[cfg(target_os = "none")]
mod window;

#[cfg(target_os = "none")]
use alloc::boxed::Box;
#[cfg(target_os = "none")]
use alloc::vec::Vec;

#[cfg(target_os = "none")]
use hk_server_api::display::{
    DISPLAY_CLICK, DISPLAY_CLICK_AT, DISPLAY_CLOSE, DISPLAY_KEY, DISPLAY_SHUTDOWN, DISPLAY_WHEEL,
    SURFACE_CREATE, SURFACE_CREATED, SURFACE_VIRT, WINDOW_CREATE, WINDOW_CREATED, pack_size,
    unpack_click_at, unpack_key, unpack_wheel,
};
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::input::{Decoder, Key};
#[cfg(target_os = "none")]
use hk_server_api::{
    exit, frame_alloc, recv, recv_data_try, send, send_handle, write, write_u64, yield_now,
};
#[cfg(target_os = "none")]
use hk_worker::Pool;
#[cfg(target_os = "none")]
use lovyangfx_fonts::Font;
#[cfg(target_os = "none")]
use lovyangfx_fonts::fonts::efont_cn_16;

#[cfg(target_os = "none")]
use document::{DocSet, DocView, HEIGHT, HELP, INDEX, TAGS, WELCOME, WIDTH};

/// 服务堆大小。
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 1024 * 1024;

/// 渲染缓冲的虚拟地址；表面在 `SURFACE_VIRT`，这里错开 2 MiB。
#[cfg(target_os = "none")]
const RENDER_VIRT: u64 = SURFACE_VIRT + 0x0020_0000;

/// 渲染工作线程栈尺寸。
#[cfg(target_os = "none")]
const RENDER_STACK: usize = 64 * 1024;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let display = hk_server_api::runtime::wait_service("hk-server-display").unwrap_or(0);
    if display == 0 {
        write(b"hk-server-holydoc: no display\n");
        exit(1)
    }

    let bytes = WIDTH * HEIGHT * 4;
    let handle = frame_alloc(bytes, SURFACE_VIRT);
    if handle == 0 || handle == u64::MAX {
        write(b"hk-server-holydoc: frame alloc failed\n");
        exit(2)
    }
    if send_handle(
        display,
        SURFACE_CREATE,
        pack_size(WIDTH as u32, HEIGHT as u32),
        handle,
    ) == u64::MAX
    {
        write(b"hk-server-holydoc: create failed\n");
        exit(3)
    }
    let (label, surface, _sender) = recv(endpoint);
    if label != SURFACE_CREATED || surface == 0 {
        write(b"hk-server-holydoc: surface rejected\n");
        exit(4)
    }
    let _ = send(display, WINDOW_CREATE, surface);
    let (label, window, _sender) = recv(endpoint);
    if label != WINDOW_CREATED || window == 0 {
        write(b"hk-server-holydoc: window rejected\n");
        exit(4)
    }

    let mut docs = DocSet::new();
    let loaded = drive::load(&mut docs, endpoint);
    docs.add_builtin();
    let font: &'static Font<'static> = Box::leak(Box::new(efont_cn_16()));
    let pixels = SURFACE_VIRT as *mut u32;
    let Some(mut view) = DocView::load(&docs, WELCOME, Vec::new(), Vec::new()) else {
        write(b"hk-server-holydoc: welcome missing\n");
        exit(5)
    };

    let render_frame = frame_alloc(bytes, RENDER_VIRT);
    if render_frame == 0 || render_frame == u64::MAX {
        write(b"hk-server-holydoc: render frame failed\n");
        exit(6)
    }
    let Some(pool) = Pool::start_with_stack(1, RENDER_STACK) else {
        write(b"hk-server-holydoc: render pool failed\n");
        exit(6)
    };

    window::update_title(display, window, &view);
    submit_render(pool, font, &view);
    wait_render(pool);
    present_render(endpoint, display, surface);
    write(b"hk-server-holydoc: render worker ok\n");
    write(b"hk-server-holydoc: ready ");
    write_u64(window);
    write(b"\n");
    write(b"hk-server-holydoc: doc ");
    write(view.id.as_bytes());
    write(b" ");
    write_u64(view.lines.len() as u64);
    write(b"\n");
    write(b"hk-server-holydoc: drive docs ");
    write_u64(loaded.documents as u64);
    write(b"\n");
    write(b"hk-server-holydoc: tags ");
    write_u64(loaded.tags as u64);
    write(b"\n");

    let mut cmdline_buffer = [0_u8; 128];
    let test = hk_server_api::boot_cmdline(&mut cmdline_buffer)
        .is_some_and(|line| line.contains("holydoc-test"));
    if test {
        // 工作线程渲染必须与主线程渲染逐像素一致。
        render::render(&view, font, pixels);
        submit_render(pool, font, &view);
        wait_render(pool);
        // SAFETY: 表面与渲染缓冲都已映射，长度均为 WIDTH * HEIGHT。
        let direct = unsafe { core::slice::from_raw_parts(pixels, WIDTH * HEIGHT) };
        // SAFETY: 同上。
        let worker =
            unsafe { core::slice::from_raw_parts(RENDER_VIRT as *const u32, WIDTH * HEIGHT) };
        if direct != worker {
            write(b"hk-server-holydoc: render worker mismatch\n");
            exit(8)
        }
        selftest::run(
            &mut view,
            &mut docs,
            font,
            pixels,
            endpoint,
            display,
            surface,
            window,
            loaded.documents,
        );
    }

    let mut keys = Decoder::new();
    let mut scratch = [0_u8; 32];
    let mut render_pending = false;
    let mut render_dirty = false;

    loop {
        if render_pending && pool.try_next().is_some() {
            present_render(endpoint, display, surface);
            render_pending = false;
            if render_dirty {
                render_dirty = false;
                submit_render(pool, font, &view);
                render_pending = true;
            }
        }

        let (label, word) = if render_pending {
            let (label, word, _sender, _length) = recv_data_try(endpoint, &mut scratch);
            if label == 0 {
                yield_now();
                continue;
            }
            (label, word)
        } else {
            let (label, word, _sender) = recv(endpoint);
            (label, word)
        };

        let mut redraw = false;
        match label {
            DISPLAY_KEY => {
                let (_, byte) = unpack_key(word);
                if let Some(key) = keys.feed(byte) {
                    redraw = match key {
                        Key::Byte(byte) => match byte {
                            b'j' => {
                                view.scroll_by(1);
                                true
                            }
                            b'k' => {
                                view.scroll_by(-1);
                                true
                            }
                            b'u' => {
                                view.scroll_by(-10);
                                true
                            }
                            b'd' => {
                                view.scroll_by(10);
                                true
                            }
                            b' ' => {
                                view.scroll_by(8);
                                true
                            }
                            b'b' => {
                                view.scroll_by(-8);
                                true
                            }
                            b'g' => {
                                view.scroll = 0;
                                true
                            }
                            b'G' => {
                                let bottom = view.max_scroll();
                                view.scroll = bottom;
                                true
                            }
                            b'n' => view.select_link(true),
                            b'N' => view.select_link(false),
                            b'\n' | b'\r' => view.open_selected(&docs),
                            b'[' | 0x08 => view.back(&docs),
                            b']' => view.advance(&docs),
                            b'i' => view.open(&docs, INDEX),
                            b't' => view.open(&docs, TAGS),
                            b'w' | b'h' => view.open(&docs, WELCOME),
                            b'?' => view.open(&docs, HELP),
                            b'q' => {
                                let _ = send(display, DISPLAY_SHUTDOWN, 0);
                                exit(0)
                            }
                            _ => false,
                        },
                        Key::Up => {
                            view.scroll_by(-1);
                            true
                        }
                        Key::Down => {
                            view.scroll_by(1);
                            true
                        }
                        Key::PageUp => {
                            view.scroll_by(-8);
                            true
                        }
                        Key::PageDown => {
                            view.scroll_by(8);
                            true
                        }
                        Key::Home => {
                            view.scroll = 0;
                            true
                        }
                        Key::End => {
                            view.scroll = view.max_scroll();
                            true
                        }
                        _ => false,
                    };
                }
            }
            DISPLAY_WHEEL => {
                let (_, delta) = unpack_wheel(word);
                if delta != 0 {
                    view.scroll_by(-(delta as isize) * 3);
                    redraw = true;
                }
            }
            DISPLAY_CLICK_AT => {
                let (_, x, y) = unpack_click_at(word);
                if let Some(target) = view.link_at(x, y) {
                    view.select(&target);
                    view.open(&docs, &target);
                    redraw = true;
                }
            }
            DISPLAY_CLICK => {}
            DISPLAY_CLOSE => exit(0),
            _ => {}
        }

        if redraw {
            window::update_title(display, window, &view);
            if render_pending {
                render_dirty = true;
            } else {
                submit_render(pool, font, &view);
                render_pending = true;
            }
        }
    }
}

/// 把当前视图快照交给渲染线程。
#[cfg(target_os = "none")]
fn submit_render(pool: &Pool, font: &'static Font<'static>, view: &DocView) {
    let snapshot = view.clone();
    let _ = pool.submit(0, move || {
        render::render(&snapshot, font, RENDER_VIRT as *mut u32);
        0
    });
}

/// 等待渲染线程完成一帧。
#[cfg(target_os = "none")]
fn wait_render(pool: &Pool) {
    while pool.try_next().is_none() {
        yield_now();
    }
}

/// 把渲染缓冲拷贝到表面并提交。
#[cfg(target_os = "none")]
fn present_render(endpoint: u64, display: u64, surface: u64) {
    // SAFETY: 渲染缓冲在启动时映射，长度为 WIDTH * HEIGHT。
    let pixels = unsafe { core::slice::from_raw_parts(RENDER_VIRT as *const u32, WIDTH * HEIGHT) };
    window::present(pixels, endpoint, display, surface);
}

hk_server_api::server_boilerplate!();
