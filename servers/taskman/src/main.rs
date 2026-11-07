//! taskman：任务管理器窗口服务。
//!
//! 提供「进程」与「性能」两页：进程页列出任务状态、CPU 占用、Arena 内存与句柄；
//! 性能页绘制 CPU 与内存历史曲线，并显示每核占用。`taskman-test` 模式下渲染两页、
//! 输出自检标记后退出。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::format;
#[cfg(target_os = "none")]
use alloc::vec;
#[cfg(target_os = "none")]
use alloc::vec::Vec;

#[cfg(target_os = "none")]
mod model;
#[cfg(target_os = "none")]
mod render;

#[cfg(target_os = "none")]
use hk_canvas::Surface;
#[cfg(target_os = "none")]
use hk_proto::display::{
    DISPLAY_CLICK_AT, DISPLAY_CLOSE, DISPLAY_KEY, DISPLAY_RESIZE, DISPLAY_SHUTDOWN, DISPLAY_WHEEL,
    SURFACE_COMMIT, SURFACE_CREATE, SURFACE_CREATED, SURFACE_FRAME_DONE, WINDOW_CREATE,
    WINDOW_CREATED, WINDOW_RESIZE, pack_size, pack_window_size, set_title, unpack_click_at,
    unpack_key, unpack_wheel, unpack_window_size,
};
#[cfg(target_os = "none")]
use hk_proto::input::{Decoder, Key};
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::root::{self, ROOT_OP_PERF};
#[cfg(target_os = "none")]
use hk_server_api::{
    CapabilityKind, MemoryClient, close_handle, exit, recv, recv_data, recv_data_try, region_unmap,
    send, send_handle, sleep_ticks, ticks, tsc, write, write_u64,
};
#[cfg(target_os = "none")]
use model::{HISTORY, MAX_CORES, Page, PerfSample, REFRESH_TICKS, SortKey};

/// 服务堆大小。
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 512 * 1024;
/// Root 控制结果缓冲长度。
#[cfg(target_os = "none")]
const BUFFER_SIZE: usize = 64 * 1024;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

/// 任务管理器状态。
#[cfg(target_os = "none")]
struct App {
    page: Page,
    sample: PerfSample,
    previous: PerfSample,
    previous_wall: u64,
    cpu_usage: u64,
    core_usage: [u64; MAX_CORES],
    cpu_history: Vec<u64>,
    memory_history: Vec<u64>,
    sort: SortKey,
    selected: usize,
    scroll: usize,
    auto: bool,
    last_refresh: u64,
    buffer: Vec<u8>,
}

#[cfg(target_os = "none")]
impl App {
    fn new() -> Self {
        Self {
            page: Page::Performance,
            sample: PerfSample::default(),
            previous: PerfSample::default(),
            previous_wall: 0,
            cpu_usage: 0,
            core_usage: [0; MAX_CORES],
            cpu_history: Vec::new(),
            memory_history: Vec::new(),
            sort: SortKey::Arena,
            selected: 0,
            scroll: 0,
            auto: true,
            last_refresh: 0,
            buffer: vec![0; BUFFER_SIZE],
        }
    }

    fn refresh(&mut self) {
        let now = ticks();
        let wall = tsc();
        let length = root::call(ROOT_OP_PERF, "", self.buffer.as_mut_slice()).unwrap_or(0);
        let end = length.min(self.buffer.len());
        let text = core::str::from_utf8(&self.buffer[..end]).unwrap_or("");
        let mut sample = model::parse_perf(text);

        for slot in self.core_usage.iter_mut() {
            *slot = 0;
        }
        let wall_delta = wall.saturating_sub(self.previous_wall);
        let cpus = sample.cpus.max(1);
        if self.previous_wall != 0 && wall_delta > 0 {
            let mut total_busy = 0_u64;
            for cpu in 0..cpus.min(MAX_CORES) {
                let before = self.previous.cpu_busy.get(cpu).copied().unwrap_or(0);
                let after = sample.cpu_busy.get(cpu).copied().unwrap_or(0);
                let busy = after.saturating_sub(before);
                total_busy = total_busy.saturating_add(busy);
                self.core_usage[cpu] = model::permille(busy, wall_delta).min(1000);
            }
            self.cpu_usage = total_busy
                .saturating_mul(1000)
                .checked_div(wall_delta.saturating_mul(cpus as u64))
                .unwrap_or(0)
                .min(1000);
            for row in &mut sample.rows {
                if let Some(before) = self.previous.rows.iter().find(|task| task.tcb == row.tcb) {
                    let delta = row.cpu_cycles.saturating_sub(before.cpu_cycles);
                    row.cpu_percent = (model::permille(delta, wall_delta) / 10).min(999);
                }
            }
        } else {
            self.cpu_usage = 0;
        }

        model::sort(&mut sample.rows, self.sort);
        if self.selected >= sample.rows.len() {
            self.selected = sample.rows.len().saturating_sub(1);
        }

        self.cpu_history.push(self.cpu_usage);
        if self.cpu_history.len() > HISTORY {
            self.cpu_history.remove(0);
        }
        let used = sample.memory_used();
        self.memory_history.push(used);
        if self.memory_history.len() > HISTORY {
            self.memory_history.remove(0);
        }

        self.previous = core::mem::replace(&mut self.sample, sample);
        self.previous_wall = wall;
        self.last_refresh = now;
    }

    fn rows(&self) -> &[model::TaskRow] {
        &self.sample.rows
    }

    fn set_sort(&mut self, key: SortKey) {
        self.sort = key;
        model::sort(&mut self.sample.rows, key);
    }

    fn select(&mut self, index: usize) {
        self.selected = if self.sample.rows.is_empty() {
            0
        } else {
            index.min(self.sample.rows.len() - 1)
        };
    }

    fn switch_page(&mut self, page: Page) {
        self.page = page;
    }

    fn ensure_visible(&mut self, visible: usize) {
        if visible == 0 {
            return;
        }
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + visible {
            self.scroll = self.selected + 1 - visible;
        }
        let max_scroll = self.sample.rows.len().saturating_sub(visible);
        if self.scroll > max_scroll {
            self.scroll = max_scroll;
        }
    }
}

/// 创建共享表面并挂到窗口，返回 `(surface, window, region, pixels)`。
#[cfg(target_os = "none")]
fn attach(
    endpoint: u64,
    display: u64,
    width: usize,
    height: usize,
) -> Option<(u64, u64, u64, *mut u32)> {
    let bytes = width.checked_mul(height)?.checked_mul(4)?;
    let (handle, region, virt) = MemoryClient::new(endpoint).map(CapabilityKind::Surface, bytes)?;
    if send_handle(
        display,
        SURFACE_CREATE,
        pack_size(width as u32, height as u32),
        handle,
    ) == u64::MAX
    {
        return None;
    }
    let (label, surface, _sender) = recv(endpoint);
    if label != SURFACE_CREATED || surface == 0 {
        return None;
    }
    let _ = send(display, WINDOW_CREATE, surface);
    let (label, window, _sender) = recv(endpoint);
    if label != WINDOW_CREATED || window == 0 {
        return None;
    }
    Some((surface, window, region, virt as *mut u32))
}

/// 为已有窗口换一块新尺寸的表面。
#[cfg(target_os = "none")]
fn resize(
    endpoint: u64,
    display: u64,
    window: u64,
    width: usize,
    height: usize,
) -> Option<(u64, *mut u32)> {
    let bytes = width.checked_mul(height)?.checked_mul(4)?;
    let (handle, region, virt) = MemoryClient::new(endpoint).map(CapabilityKind::Surface, bytes)?;
    if send_handle(
        display,
        WINDOW_RESIZE,
        pack_window_size(window, width as u32, height as u32),
        handle,
    ) == u64::MAX
    {
        let _ = close_handle(handle);
        return None;
    }
    Some((region, virt as *mut u32))
}

/// 提交并等待显示服务完成合成。
#[cfg(target_os = "none")]
fn commit(endpoint: u64, display: u64, surface: u64) {
    let _ = send(display, SURFACE_COMMIT, surface);
    let mut buffer = [0_u8; 2048];
    loop {
        let (label, word, _sender, _length) = recv_data(endpoint, &mut buffer);
        match label {
            SURFACE_FRAME_DONE if word == surface => return,
            DISPLAY_CLOSE => exit(0),
            _ => {}
        }
    }
}

/// 绘制并提交一帧。
#[cfg(target_os = "none")]
#[allow(clippy::too_many_arguments)]
fn show(
    app: &App,
    width: usize,
    height: usize,
    pixels: *mut u32,
    endpoint: u64,
    display: u64,
    surface: u64,
    window: u64,
) {
    let title = format!("任务管理器 · {}", app.page.title());
    set_title(display, window, &title);
    let view = render::View {
        page: app.page,
        sample: &app.sample,
        cpu_usage: app.cpu_usage,
        cpu_history: &app.cpu_history,
        core_usage: &app.core_usage,
        memory_history: &app.memory_history,
        sort: app.sort,
        selected: app.selected,
        scroll: app.scroll,
        auto_refresh: app.auto,
    };
    // SAFETY: 表面帧已映射，长度为 width * height。
    let frame = unsafe { Surface::from_raw(pixels, width, height, width) };
    render::render(&view, width, height, &frame);
    commit(endpoint, display, surface);
}

/// 输出自检标记。
#[cfg(target_os = "none")]
fn log_test(app: &App, window: u64) {
    write(b"hk-server-taskman: ready ");
    write_u64(window);
    write(b"\n");
    write(b"hk-server-taskman: cpus ");
    write_u64(app.sample.cpus as u64);
    write(b"\n");
    write(b"hk-server-taskman: tasks ");
    write_u64(app.sample.tasks);
    write(b"\n");
    write(b"hk-server-taskman: rows ");
    write_u64(app.sample.rows.len() as u64);
    write(b"\n");
    write(b"hk-server-taskman: cpu ");
    write_u64(app.cpu_usage);
    write(b"\n");
    for row in app.rows().iter().take(8) {
        write(b"hk-server-taskman: task ");
        write(row.name.as_bytes());
        write(b" ");
        write(row.state.as_bytes());
        write(b"\n");
    }
    let busy: u64 = app.sample.cpu_busy.iter().sum();
    write(b"hk-server-taskman: busy ");
    write_u64(busy);
    write(b"\n");
    write(b"hk-server-taskman: memory ");
    write_u64(app.sample.memory_used());
    write(b" ");
    write_u64(app.sample.memory_total());
    write(b" ");
    write_u64(app.sample.system.peak);
    write(b"\n");
    write(b"hk-server-taskman: memory-kinds ");
    write_u64(app.sample.system.heap);
    write(b" ");
    write_u64(app.sample.system.page_table);
    write(b" ");
    write_u64(app.sample.system.cow);
    write(b" ");
    write_u64(app.sample.system.restore);
    write(b" ");
    write_u64(app.sample.system.image);
    write(b" ");
    write_u64(app.sample.system.dma);
    write(b" ");
    write_u64(app.sample.system.other);
    write(b"\n");
    write(b"hk-server-taskman: render ok\n");
    write(b"hk-server-taskman: done\n");
}

/// 处理选中、排序与页面切换；返回是否需要重绘。
#[cfg(target_os = "none")]
fn handle_key(app: &mut App, key: Key, visible: usize) -> bool {
    match key {
        Key::Byte(b'\t') => {
            app.page = match app.page {
                Page::Processes => Page::Performance,
                Page::Performance => Page::Processes,
            };
            true
        }
        Key::Byte(b'1') => {
            app.switch_page(Page::Processes);
            true
        }
        Key::Byte(b'2') => {
            app.switch_page(Page::Performance);
            true
        }
        Key::Up | Key::Byte(b'k') => {
            app.select(app.selected.saturating_sub(1));
            true
        }
        Key::Down | Key::Byte(b'j') => {
            app.select(app.selected.saturating_add(1));
            true
        }
        Key::PageUp | Key::Byte(b'u') => {
            app.select(app.selected.saturating_sub(visible));
            true
        }
        Key::PageDown | Key::Byte(b'd') => {
            app.select(app.selected.saturating_add(visible));
            true
        }
        Key::Home | Key::Byte(b'g') => {
            app.select(0);
            true
        }
        Key::End | Key::Byte(b'G') => {
            app.select(app.sample.rows.len().saturating_sub(1));
            true
        }
        Key::Byte(b'r') => {
            app.refresh();
            true
        }
        Key::Byte(b' ') => {
            app.auto = !app.auto;
            app.last_refresh = ticks();
            true
        }
        Key::Byte(byte) => match SortKey::from_byte(byte) {
            Some(sort) => {
                app.set_sort(sort);
                true
            }
            None => false,
        },
        _ => false,
    }
}

/// 滚轮移动选中行；正值向上。
#[cfg(target_os = "none")]
fn scroll_by(app: &mut App, delta: i32) {
    if delta > 0 {
        app.select(app.selected.saturating_sub(delta as usize));
    } else if delta < 0 {
        app.select(app.selected.saturating_add(delta.unsigned_abs() as usize));
    }
}

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let display = hk_server_api::runtime::wait_service("hk-server-display").unwrap_or(0);
    if display == 0 {
        write(b"hk-server-taskman: no display\n");
        exit(1)
    }

    let mut width = render::WIDTH;
    let mut height = render::HEIGHT;
    let Some((surface, window, mut surface_region, mut pixels)) =
        attach(endpoint, display, width, height)
    else {
        write(b"hk-server-taskman: attach failed\n");
        exit(2)
    };

    let mut app = App::new();
    app.refresh();
    show(
        &app, width, height, pixels, endpoint, display, surface, window,
    );

    let mut cmdline_buffer = [0_u8; 128];
    let test = hk_server_api::boot_cmdline(&mut cmdline_buffer)
        .is_some_and(|line| line.contains("taskman-test"));
    if test {
        // 空闲采样一个刷新周期，验证 CPU 占用率不会因为轮询而持续打满。
        let _ = sleep_ticks(REFRESH_TICKS);
        app.refresh();
        app.switch_page(Page::Processes);
        show(
            &app, width, height, pixels, endpoint, display, surface, window,
        );
        app.switch_page(Page::Performance);
        show(
            &app, width, height, pixels, endpoint, display, surface, window,
        );
        log_test(&app, window);
        let _ = send(display, DISPLAY_SHUTDOWN, 0);
        exit(0)
    }

    let mut keys = Decoder::new();
    let mut buffer = [0_u8; 2048];
    loop {
        let mut dirty = false;
        loop {
            let (label, word, _sender, _length) = recv_data_try(endpoint, &mut buffer);
            if label == 0 {
                break;
            }
            match label {
                DISPLAY_KEY => {
                    let (_, byte) = unpack_key(word);
                    let Some(key) = keys.feed(byte) else {
                        continue;
                    };
                    if let Key::Byte(b'q') = key {
                        let _ = send(display, DISPLAY_SHUTDOWN, 0);
                        exit(0)
                    }
                    let visible = render::visible_rows(height);
                    if handle_key(&mut app, key, visible) {
                        dirty = true;
                    }
                }
                DISPLAY_CLICK_AT => {
                    let (_, x, y) = unpack_click_at(word);
                    if let Some(page) = render::tab_hit(x, y) {
                        app.switch_page(page);
                        dirty = true;
                    } else if app.page == Page::Processes
                        && let Some(index) = render::row_hit(y, app.scroll, app.sample.rows.len())
                    {
                        app.select(index);
                        dirty = true;
                    }
                }
                DISPLAY_RESIZE => {
                    let (_, new_width, new_height) = unpack_window_size(word);
                    let new_width = new_width as usize;
                    let new_height = new_height as usize;
                    if new_width == 0 || new_height == 0 {
                        continue;
                    }
                    if new_width == width && new_height == height {
                        continue;
                    }
                    match resize(endpoint, display, window, new_width, new_height) {
                        Some((region, virt)) => {
                            let old_region = surface_region;
                            width = new_width;
                            height = new_height;
                            surface_region = region;
                            pixels = virt;
                            let _ = region_unmap(old_region);
                            dirty = true;
                        }
                        None => write(b"hk-server-taskman: resize failed\n"),
                    }
                }
                DISPLAY_WHEEL => {
                    let (_, delta) = unpack_wheel(word);
                    scroll_by(&mut app, delta);
                    dirty = true;
                }
                SURFACE_FRAME_DONE => {}
                DISPLAY_CLOSE => exit(0),
                _ => {}
            }
        }

        let now = ticks();
        if app.auto && now.saturating_sub(app.last_refresh) >= REFRESH_TICKS {
            app.refresh();
            dirty = true;
        }
        if dirty {
            let visible = render::visible_rows(height);
            app.ensure_visible(visible);
            show(
                &app, width, height, pixels, endpoint, display, surface, window,
            );
        }
        let _ = sleep_ticks(1);
    }
}

hk_server_api::server_boilerplate!();
