//! holydoc：最小活文档窗口服务。
//!
//! 创建 Display 窗口，把静态文档渲染到共享表面，支持键盘滚动与关闭；
//! `holydoc-test` 启动模式下渲染、滚动一次后关闭 Display 并退出，供冒烟使用。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use hk_server_api::display::{
    DISPLAY_CLICK, DISPLAY_CLOSE, DISPLAY_COMMIT, DISPLAY_CREATE_WINDOW, DISPLAY_FRAME_DONE,
    DISPLAY_KEY, DISPLAY_SHUTDOWN, DISPLAY_WINDOW_CREATED, SURFACE_VIRT, pack_size, set_title,
    unpack_key,
};
#[cfg(target_os = "none")]
use hk_server_api::{exit, frame_alloc, recv, send, send_handle, write, write_u64};
#[cfg(target_os = "none")]
use lovyangfx_fonts::Font;
#[cfg(target_os = "none")]
use lovyangfx_fonts::fonts::efont_cn_16;

/// 窗口宽度。
#[cfg(target_os = "none")]
const WIDTH: usize = 760;
/// 窗口高度。
#[cfg(target_os = "none")]
const HEIGHT: usize = 520;
/// 行高。
#[cfg(target_os = "none")]
const LINE_HEIGHT: usize = 22;
/// 页面留白。
#[cfg(target_os = "none")]
const MARGIN: usize = 24;

/// 纸张背景色。
#[cfg(target_os = "none")]
const PAPER: u32 = 0x00F6_F1E7;
/// 正文颜色。
#[cfg(target_os = "none")]
const INK: u32 = 0x0024_2A30;
/// 标题颜色。
#[cfg(target_os = "none")]
const HEADER: u32 = 0x0040_6080;
/// 链接颜色。
#[cfg(target_os = "none")]
const LINK: u32 = 0x0030_6090;

/// 静态文档内容。
#[cfg(target_os = "none")]
const DOCUMENT: &[(&str, u32)] = &[
    ("HolyDoc", HEADER),
    ("原生活文档 · 最小窗口", HEADER),
    ("", INK),
    ("这是运行在 HolyKernel 上的 HolyDoc 窗口。", INK),
    ("文档由服务渲染到共享表面，", INK),
    ("再由 Display Server 合成到帧缓冲。", INK),
    ("", INK),
    ("当前能力", HEADER),
    ("  · 段落、标题与行内链接的静态排版", INK),
    ("  · j / k 键盘滚动", INK),
    ("  · 标题栏关闭按钮退出", INK),
    ("", INK),
    ("窗口与输入", HEADER),
    ("  · 共享 Frame 经句柄转移给 Display", INK),
    ("  · 键盘事件按焦点经 IPC 路由", INK),
    ("  · 任务栏、缩放与主题由 Display 管理", INK),
    ("", INK),
    ("后续里程碑", HEADER),
    ("  · Markdown 解析与增量布局", INK),
    ("  · 链接跳转与文档历史", INK),
    ("  · @exec 结果嵌入", INK),
    ("  · 可缩放字体与富文本属性", INK),
    ("", INK),
    ("链接", HEADER),
    ("  architecture.html", LINK),
    ("  crates.html", LINK),
    ("  roadmap.html", LINK),
    ("", INK),
    ("按 j 向下滚动，按 k 向上滚动，", INK),
    ("或点击标题栏关闭按钮退出。", INK),
];

/// 文档视图。
#[cfg(target_os = "none")]
struct Doc {
    pixels: *mut u32,
    font: Font<'static>,
    scroll: usize,
}

#[cfg(target_os = "none")]
impl Doc {
    fn max_scroll(&self) -> usize {
        (DOCUMENT.len() * LINE_HEIGHT).saturating_sub(HEIGHT.saturating_sub(MARGIN * 2))
    }

    fn scroll_by(&mut self, delta: isize) {
        let next = self.scroll as isize + delta * LINE_HEIGHT as isize;
        self.scroll = next.clamp(0, self.max_scroll() as isize) as usize;
    }

    fn render(&self) {
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                self.put(x, y, PAPER);
            }
        }
        for (index, (line, color)) in DOCUMENT.iter().enumerate() {
            let baseline = MARGIN as isize + (index * LINE_HEIGHT) as isize - self.scroll as isize;
            if baseline + 16 < 0 || baseline >= HEIGHT as isize {
                continue;
            }
            self.draw_line(line, baseline, *color);
        }
    }

    fn draw_line(&self, line: &str, baseline: isize, color: u32) {
        let mut pen = MARGIN as i32;
        let limit = WIDTH.saturating_sub(MARGIN) as i32;
        for character in line.chars() {
            let Some(glyph) = self.font.glyph(character) else {
                pen += 8;
                continue;
            };
            let width = if glyph.metric().xadvance.max(0) as usize > 8 {
                16
            } else {
                8
            };
            if pen + width as i32 > limit {
                break;
            }
            glyph.for_each_pixel(|x, y| {
                let pixel_x = pen + x;
                let pixel_y = baseline as i32 + y;
                if pixel_x >= 0 && pixel_y >= 0 {
                    self.put(pixel_x as usize, pixel_y as usize, color);
                }
            });
            pen += width as i32;
        }
    }

    fn put(&self, x: usize, y: usize, color: u32) {
        if x >= WIDTH || y >= HEIGHT {
            return;
        }
        // SAFETY: 共享表面已按 WIDTH×HEIGHT 映射，坐标已裁剪。
        unsafe {
            self.pixels.add(y * WIDTH + x).write_volatile(color);
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
    set_title(display, window, "HolyDoc");

    let mut doc = Doc {
        pixels: SURFACE_VIRT as *mut u32,
        font: efont_cn_16(),
        scroll: 0,
    };
    doc.render();
    commit(endpoint, display, window);
    write(b"hk-server-holydoc: ready ");
    write_u64(window);
    write(b"\n");

    let mut cmdline_buffer = [0_u8; 128];
    let test = hk_server_api::boot_cmdline(&mut cmdline_buffer)
        .is_some_and(|line| line.contains("holydoc-test"));
    if test {
        doc.scroll_by(5);
        doc.render();
        commit(endpoint, display, window);
        write(b"hk-server-holydoc: scroll ");
        write_u64(doc.scroll as u64);
        write(b"\n");
        write(b"hk-server-holydoc: done\n");
        let _ = send(display, DISPLAY_SHUTDOWN, 0);
        exit(0)
    }

    loop {
        let (label, word, _sender) = recv(endpoint);
        match label {
            DISPLAY_KEY => {
                let (_, byte) = unpack_key(word);
                match byte {
                    b'j' => {
                        doc.scroll_by(1);
                        doc.render();
                        commit(endpoint, display, window);
                        write(b"hk-server-holydoc: scroll ");
                        write_u64(doc.scroll as u64);
                        write(b"\n");
                    }
                    b'k' => {
                        doc.scroll_by(-1);
                        doc.render();
                        commit(endpoint, display, window);
                    }
                    b'q' => {
                        let _ = send(display, DISPLAY_SHUTDOWN, 0);
                        exit(0)
                    }
                    _ => {}
                }
            }
            DISPLAY_CLICK => write(b"hk-server-holydoc: click\n"),
            DISPLAY_CLOSE => exit(0),
            _ => {}
        }
    }
}

hk_server_api::server_boilerplate!();
