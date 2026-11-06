//! workbench：原生一体化工作台。
//!
//! 左侧浏览 `H:` 卷文档树，中间编辑文档，底部输出构建、运行与调试结果。
//! 文档经原生 Holy 协议整篇读写；构建统计文档指令，运行求值 `@exec`。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![cfg_attr(all(test, not(target_os = "none")), allow(dead_code))]

#[cfg(any(target_os = "none", test))]
extern crate alloc;

#[cfg(any(target_os = "none", test))]
mod model;

#[cfg(target_os = "none")]
mod render;
#[cfg(target_os = "none")]
mod selftest;

#[cfg(target_os = "none")]
use alloc::boxed::Box;
#[cfg(target_os = "none")]
use alloc::format;
#[cfg(target_os = "none")]
use alloc::string::String;

#[cfg(target_os = "none")]
use hk_canvas::Surface;
#[cfg(target_os = "none")]
use hk_font::Font;
#[cfg(target_os = "none")]
use hk_server_api::display::{
    DISPLAY_CLICK_AT, DISPLAY_CLOSE, DISPLAY_KEY, DISPLAY_SHUTDOWN, DISPLAY_WHEEL, SURFACE_COMMIT,
    SURFACE_CREATE, SURFACE_CREATED, SURFACE_FRAME_DONE, SURFACE_VIRT, WINDOW_CREATE,
    WINDOW_CREATED, pack_size, set_title, unpack_click_at, unpack_key, unpack_wheel,
};
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::input::{Decoder, Key};
#[cfg(target_os = "none")]
use hk_server_api::{
    exit, fault_subscribe, frame_alloc, recv, recv_data, send, send_handle, write, write_u64,
};

#[cfg(target_os = "none")]
use model::{EditorKey, Focus, Model, VISIBLE_LINES, line_text};
#[cfg(target_os = "none")]
use render::{BODY_TOP, HEIGHT, NAV_TOP, OUTPUT_TOP, ROW, SIDEBAR, TEXT_LEFT, WIDTH, render};

/// 服务堆大小。
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 1024 * 1024;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

/// 提交表面并等待合成完成。
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

/// 刷新标题、绘制并提交。
#[cfg(target_os = "none")]
pub(crate) fn show(
    model: &Model,
    font: &Font,
    pixels: *mut u32,
    endpoint: u64,
    display: u64,
    surface: u64,
    window: u64,
) {
    let title = match &model.editor {
        Some(editor) => format!("Workbench: {}", editor.title),
        None => String::from("Workbench"),
    };
    set_title(display, window, &title);
    // SAFETY: 表面帧已映射，长度为 WIDTH * HEIGHT。
    let frame = unsafe { Surface::from_raw(pixels, WIDTH, HEIGHT, WIDTH) };
    render(model, font, &frame);
    commit(endpoint, display, surface);
}

/// 处理点击：命中文档树或编辑器。
#[cfg(target_os = "none")]
fn click(model: &mut Model, font: &Font, x: u32, y: u32) {
    let x = x as usize;
    let y = y as usize;
    if x < SIDEBAR && y >= NAV_TOP {
        let index = model.nav_scroll + (y - NAV_TOP) / ROW;
        if index < model.entries.len() {
            model.open(index);
        }
        model.focus = Focus::Navigator;
        return;
    }
    if x >= SIDEBAR && y >= BODY_TOP && y < OUTPUT_TOP {
        model.focus = Focus::Editor;
        let position = model.editor.as_ref().map(|editor| {
            let line = (y - BODY_TOP) / ROW + editor.scroll_line;
            let line = line.min(editor.text.split('\n').count().saturating_sub(1));
            let target = x.saturating_sub(TEXT_LEFT);
            let mut column = 0;
            let mut width = 0;
            for character in line_text(&editor.text, line).chars() {
                let advance = font.advance(character) as usize;
                if width + advance / 2 >= target {
                    break;
                }
                width += advance;
                column += 1;
            }
            (line, editor.scroll_col + column)
        });
        if let Some((line, column)) = position {
            model.set_cursor(line, column);
        }
    }
}

/// 处理按键；返回是否需要重绘。
#[cfg(target_os = "none")]
fn handle_key(model: &mut Model, key: Key, display: u64) -> bool {
    match key {
        Key::Byte(0x11) => {
            let _ = send(display, DISPLAY_SHUTDOWN, 0);
            exit(0)
        }
        Key::Byte(0x13) => {
            model.save();
            true
        }
        Key::Byte(0x02) => {
            model.build();
            true
        }
        Key::Byte(0x12) => {
            match model.focus {
                Focus::Navigator => {
                    model.refresh();
                }
                Focus::Editor => {
                    model.run();
                }
            }
            true
        }
        Key::Byte(0x04) => {
            model.debug();
            true
        }
        Key::Byte(b'\t') => {
            model.toggle_focus();
            true
        }
        Key::Byte(b'\n') | Key::Byte(b'\r') => {
            match model.focus {
                Focus::Navigator => {
                    model.enter();
                }
                Focus::Editor => model.insert('\n'),
            }
            true
        }
        Key::Byte(0x7F) | Key::Byte(0x08) => {
            if model.focus == Focus::Editor {
                model.backspace();
                true
            } else {
                false
            }
        }
        Key::Byte(byte) if (0x20..0x7F).contains(&byte) => {
            match model.focus {
                Focus::Editor => model.insert(byte as char),
                Focus::Navigator => match byte {
                    b'j' => model.select_next(),
                    b'k' => model.select_previous(),
                    _ => {}
                },
            }
            true
        }
        Key::Byte(_) => false,
        Key::Up => {
            match model.focus {
                Focus::Navigator => model.select_previous(),
                Focus::Editor => model.move_editor(EditorKey::Up),
            }
            true
        }
        Key::Down => {
            match model.focus {
                Focus::Navigator => model.select_next(),
                Focus::Editor => model.move_editor(EditorKey::Down),
            }
            true
        }
        Key::Left => {
            if model.focus == Focus::Editor {
                model.move_editor(EditorKey::Left);
                true
            } else {
                false
            }
        }
        Key::Right => {
            if model.focus == Focus::Editor {
                model.move_editor(EditorKey::Right);
                true
            } else {
                false
            }
        }
        Key::Home => {
            if model.focus == Focus::Editor {
                model.move_editor(EditorKey::Home);
                true
            } else {
                false
            }
        }
        Key::End => {
            if model.focus == Focus::Editor {
                model.move_editor(EditorKey::End);
                true
            } else {
                false
            }
        }
        Key::PageUp => {
            match model.focus {
                Focus::Navigator => model.scroll_nav(-(VISIBLE_LINES as isize)),
                Focus::Editor => model.scroll_editor(-(VISIBLE_LINES as isize)),
            }
            true
        }
        Key::PageDown => {
            match model.focus {
                Focus::Navigator => model.scroll_nav(VISIBLE_LINES as isize),
                Focus::Editor => model.scroll_editor(VISIBLE_LINES as isize),
            }
            true
        }
        Key::Insert => false,
        Key::Delete => {
            if model.focus == Focus::Editor {
                model.delete();
                true
            } else {
                false
            }
        }
    }
}

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    write(b"hk-server-workbench: start\n");
    let display = hk_server_api::runtime::wait_service("hk-server-display").unwrap_or(0);
    if display == 0 {
        write(b"hk-server-workbench: no display\n");
        exit(1)
    }
    let bytes = WIDTH * HEIGHT * 4;
    let handle = frame_alloc(bytes, SURFACE_VIRT);
    if handle == 0 || handle == u64::MAX {
        write(b"hk-server-workbench: frame alloc failed\n");
        exit(2)
    }
    if send_handle(
        display,
        SURFACE_CREATE,
        pack_size(WIDTH as u32, HEIGHT as u32),
        handle,
    ) == u64::MAX
    {
        write(b"hk-server-workbench: create failed\n");
        exit(3)
    }
    let (label, surface, _sender) = recv(endpoint);
    if label != SURFACE_CREATED || surface == 0 {
        write(b"hk-server-workbench: surface rejected\n");
        exit(4)
    }
    let _ = send(display, WINDOW_CREATE, surface);
    let (label, window, _sender) = recv(endpoint);
    if label != WINDOW_CREATED || window == 0 {
        write(b"hk-server-workbench: window rejected\n");
        exit(4)
    }

    let font = hk_canvas::default_font();
    let pixels = SURFACE_VIRT as *mut u32;
    let _ = fault_subscribe(endpoint);
    let mut model = Box::new(Model::new(endpoint));
    show(&model, font, pixels, endpoint, display, surface, window);
    write(b"hk-server-workbench: ready ");
    write_u64(window);
    write(b"\n");
    write(b"hk-server-workbench: documents ");
    write_u64(model.entries.len() as u64);
    write(b"\n");

    let mut cmdline_buffer = [0_u8; 128];
    let test = hk_server_api::boot_cmdline(&mut cmdline_buffer)
        .is_some_and(|line| line.contains("workbench-test"));
    if test {
        selftest::run(&mut model, font, pixels, endpoint, display, surface, window);
    }

    let mut decoder = Decoder::new();
    let mut buffer = [0_u8; 2048];
    loop {
        let (label, word, _sender, length) = recv_data(endpoint, &mut buffer);
        if label == hk_server_api::FAULT_EVENT {
            hk_debug::ingest(label, &buffer[..length]);
            continue;
        }
        let redraw = match label {
            DISPLAY_KEY => {
                let (_, byte) = unpack_key(word);
                match decoder.feed(byte) {
                    Some(key) => handle_key(&mut model, key, display),
                    None => false,
                }
            }
            DISPLAY_WHEEL => {
                let (_, delta) = unpack_wheel(word);
                if delta != 0 {
                    match model.focus {
                        Focus::Navigator => model.scroll_nav(-(delta as isize) * 2),
                        Focus::Editor => model.scroll_editor(-(delta as isize) * 3),
                    }
                }
                delta != 0
            }
            DISPLAY_CLICK_AT => {
                let (_, x, y) = unpack_click_at(word);
                click(&mut model, font, x, y);
                true
            }
            DISPLAY_CLOSE => exit(0),
            _ => false,
        };
        if redraw {
            show(&model, font, pixels, endpoint, display, surface, window);
        }
    }
}

hk_server_api::server_boilerplate!();
