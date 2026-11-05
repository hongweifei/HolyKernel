//! holydoc：原生活文档窗口服务。
//!
//! 服务入口与协议循环；内置页面、磁盘文档与布局在 `document`，绘制在
//! `render`，`@exec` 命令在 `exec`。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
mod document;
#[cfg(target_os = "none")]
mod exec;
#[cfg(target_os = "none")]
mod render;

#[cfg(target_os = "none")]
use alloc::boxed::Box;
#[cfg(target_os = "none")]
use alloc::format;
#[cfg(target_os = "none")]
use alloc::string::String;
#[cfg(target_os = "none")]
use alloc::vec;
#[cfg(target_os = "none")]
use alloc::vec::Vec;

#[cfg(target_os = "none")]
use hk_server_api::display::{
    DISPLAY_CLICK, DISPLAY_CLICK_AT, DISPLAY_CLOSE, DISPLAY_COMMIT, DISPLAY_CREATE_WINDOW,
    DISPLAY_FRAME_DONE, DISPLAY_KEY, DISPLAY_SHUTDOWN, DISPLAY_WINDOW_CREATED, SURFACE_VIRT,
    pack_size, set_title, unpack_click_at, unpack_key,
};
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::vfs::{OPEN_READ, VfsClient};
#[cfg(target_os = "none")]
use hk_server_api::{
    exit, frame_alloc, recv, send, send_handle, service_endpoint, write, write_u64,
};
#[cfg(target_os = "none")]
use lovyangfx_fonts::Font;
#[cfg(target_os = "none")]
use lovyangfx_fonts::fonts::efont_cn_16;

#[cfg(target_os = "none")]
use document::{DISK_PREFIX, DocSet, DocView, HEIGHT, INDEX, WELCOME, WIDTH, title_of};
#[cfg(target_os = "none")]
use render::render;

/// 服务堆大小。
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 1024 * 1024;

/// 磁盘文档目录候选；ext4 既可作主根，也可挂载在 `/disk`。
#[cfg(target_os = "none")]
const DOC_DIRS: &[&str] = &["/docs", "/disk/docs"];

/// 单页文档读取上限。
#[cfg(target_os = "none")]
const DOC_MAX: usize = 48 * 1024;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

/// 从 VFS 目录加载 `.md` 文档到文档集。
#[cfg(target_os = "none")]
fn load_disk_docs(set: &mut DocSet, endpoint: u64) {
    if service_endpoint("hk-server-vfs") == 0 {
        write(b"hk-server-holydoc: no vfs\n");
        return;
    }
    let mut client = Box::new(VfsClient::new("hk-server-vfs", endpoint));
    let mut names = [0_u8; 4096];
    for directory in DOC_DIRS {
        let Ok(length) = client.readdir(directory, &mut names) else {
            continue;
        };
        let length = length.min(names.len());
        let Ok(list) = core::str::from_utf8(&names[..length]) else {
            continue;
        };
        for name in list.lines() {
            let Some(stem) = name.strip_suffix(".md") else {
                continue;
            };
            if stem.is_empty() {
                continue;
            }
            let path = format!("{directory}/{name}");
            let Ok(handle) = client.open(&path, OPEN_READ) else {
                continue;
            };
            let mut body = vec![0_u8; DOC_MAX];
            if let Ok(length) = client.read(handle, 0, &mut body) {
                let body = String::from_utf8_lossy(&body[..length]).into_owned();
                let title = title_of(&body).unwrap_or_else(|| String::from(stem));
                let id = format!("{DISK_PREFIX}{stem}");
                if set.add(id, title, body) {
                    write(b"hk-server-holydoc: disk ");
                    write(name.as_bytes());
                    write(b"\n");
                }
            }
            let _ = client.close(handle);
        }
    }
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

/// 刷新标题、渲染并提交。
#[cfg(target_os = "none")]
fn apply(
    view: &DocView,
    font: &Font<'static>,
    pixels: *mut u32,
    endpoint: u64,
    display: u64,
    window: u64,
) {
    let title = format!("HolyDoc: {}", view.title);
    set_title(display, window, &title);
    render(view, font, pixels);
    commit(endpoint, display, window);
}

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
        DISPLAY_CREATE_WINDOW,
        pack_size(WIDTH as u32, HEIGHT as u32),
        handle,
    ) == u64::MAX
    {
        write(b"hk-server-holydoc: create failed\n");
        exit(3)
    }
    let (label, window, _sender) = recv(endpoint);
    if label != DISPLAY_WINDOW_CREATED || window == 0 {
        write(b"hk-server-holydoc: window rejected\n");
        exit(4)
    }

    let mut docs = DocSet::new();
    load_disk_docs(&mut docs, endpoint);
    let font = efont_cn_16();
    let pixels = SURFACE_VIRT as *mut u32;
    let Some(mut view) = DocView::load(&docs, WELCOME, Vec::new()) else {
        write(b"hk-server-holydoc: welcome missing\n");
        exit(5)
    };
    apply(&view, &font, pixels, endpoint, display, window);
    write(b"hk-server-holydoc: ready ");
    write_u64(window);
    write(b"\n");
    write(b"hk-server-holydoc: doc ");
    write(view.id.as_bytes());
    write(b" ");
    write_u64(view.lines.len() as u64);
    write(b"\n");
    write(b"hk-server-holydoc: docs ");
    write_u64(docs.disk_count() as u64);
    write(b"\n");

    let mut cmdline_buffer = [0_u8; 128];
    let test = hk_server_api::boot_cmdline(&mut cmdline_buffer)
        .is_some_and(|line| line.contains("holydoc-test"));
    if test {
        if let Some((x, y, target)) = view.first_link() {
            write(b"hk-server-holydoc: click ");
            write(target.as_bytes());
            write(b"\n");
            if view.link_at(x, y).is_some() && view.open(&docs, &target) {
                apply(&view, &font, pixels, endpoint, display, window);
            }
        }
        if docs.disk_count() > 0
            && let Some(target) = docs.resolve("architecture.md")
            && view.open(&docs, &target)
        {
            apply(&view, &font, pixels, endpoint, display, window);
        }
        view.scroll_by(5);
        apply(&view, &font, pixels, endpoint, display, window);
        write(b"hk-server-holydoc: scroll ");
        write_u64(view.scroll as u64);
        write(b"\n");
        if view.back(&docs) {
            apply(&view, &font, pixels, endpoint, display, window);
        }
        write(b"hk-server-holydoc: done\n");
        let _ = send(display, DISPLAY_SHUTDOWN, 0);
        exit(0)
    }

    loop {
        let (label, word, _sender) = recv(endpoint);
        match label {
            DISPLAY_KEY => {
                let (_, byte) = unpack_key(word);
                let redraw = match byte {
                    b'j' => {
                        view.scroll_by(1);
                        true
                    }
                    b'k' => {
                        view.scroll_by(-1);
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
                    b'd' => view.open(&docs, INDEX),
                    b'w' | b'h' => view.open(&docs, WELCOME),
                    0x08 => view.back(&docs),
                    b'q' => {
                        let _ = send(display, DISPLAY_SHUTDOWN, 0);
                        exit(0)
                    }
                    _ => false,
                };
                if redraw {
                    apply(&view, &font, pixels, endpoint, display, window);
                }
            }
            DISPLAY_CLICK_AT => {
                let (_, x, y) = unpack_click_at(word);
                if let Some(target) = view.link_at(x, y)
                    && view.open(&docs, &target)
                {
                    apply(&view, &font, pixels, endpoint, display, window);
                }
            }
            DISPLAY_CLICK => {}
            DISPLAY_CLOSE => exit(0),
            _ => {}
        }
    }
}

hk_server_api::server_boilerplate!();
