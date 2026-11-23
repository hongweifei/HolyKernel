//! Display 合成器：持有窗口、表面与焦点状态，按消息分派到各处理模块。

use hk_font::Font;
use hk_proto::display::{
    DISPLAY_RESIZE, DISPLAY_RESTART, DISPLAY_SET_TITLE, DISPLAY_SHUTDOWN, SEAT_CLIPBOARD,
    SEAT_FOCUS, SEAT_GET_CLIPBOARD, SEAT_SET_CLIPBOARD, SURFACE_COMMIT, SURFACE_CREATE,
    SURFACE_DAMAGE, SURFACE_DESTROY, SURFACE_SET_CURSOR, SURFACE_SET_INPUT_REGION, TITLE_MAX,
    WINDOW_CREATE, WINDOW_RESIZE, pack_window_size,
};
use hk_proto::input::{INPUT_BUTTON, INPUT_KEY, INPUT_POINTER, INPUT_WHEEL};
use hk_user_api::{
    close_handle, console_read, exit, format::Line, recv_data_handle_try, send_data, sleep_ticks,
    tsc, tsc_frequency, write, write_u64,
};

use crate::seat::Seat;
use crate::surface::{MAX_SURFACES, Surface};
use crate::window::{Drag, MAX_WINDOWS, Resize, Screen, Window};

mod input;
mod render;
pub(crate) mod surface;
mod window;

use render::compose_region;

pub(crate) use render::{
    compose_window_region, damage_taskbar, damage_title, damage_window, draw_desktop,
};
pub(crate) use surface::set_scanout;

/// 合成器的窗口、表面与输入状态。
pub(crate) struct Compositor {
    endpoint: u64,
    screen: Screen,
    font: &'static Font,
    windows: [Option<Window>; MAX_WINDOWS],
    surfaces: [Option<Surface>; MAX_SURFACES],
    next_window_id: u64,
    next_surface_id: u64,
    focus: u64,
    seat: Seat,
    drag: Option<Drag>,
    resize: Option<Resize>,
    driver_input: bool,
    display_test: bool,
    /// GPU 扫描输出接管；存在时每帧提交后让驱动 flush。
    scanout: Option<crate::scanout::Scanout>,
    /// 接管扫描输出期间缓存的客户端消息，启动后按序重放。
    pending: alloc::vec::Vec<crate::scanout::Pending>,
    benchmarked: bool,
    benchmark_at: u64,
    resize_retry_at: u64,
    /// 是否有尚未提交到扫描输出的像素。
    scanout_pending: bool,
    /// 下一次允许 flush 扫描输出的时间（TSC）。
    scanout_flush_at: u64,
    /// 扫描输出帧预算（cycles）。
    scanout_period: u64,
    /// 上一次实际画到屏幕上的光标矩形；用于形状变化/位置移动时精确擦除。
    cursor_drawn: Option<(usize, usize, usize, usize)>,
}

/// 客户端表面尺寸与窗口不一致时，补发缩放通知的间隔（微秒）。
const RESIZE_RETRY_MICROS: u64 = 200_000;

/// 扫描输出提交的最小间隔（微秒）：每次 flush 都要整帧 TRANSFER_TO_HOST_2D
/// 并同步等设备回复，按 60 Hz 合并 damage 才能避免打满 virtio 控制队列。
const SCANOUT_FRAME_MICROS: u64 = 16_666;

/// 单批最多处理的消息数；内核端点队列容量为 64，批次在队列取空时提前结束。
const BATCH_LIMIT: usize = 64;

impl Compositor {
    /// 建立合成器状态并绘制初始任务栏。
    pub(crate) fn new(
        endpoint: u64,
        screen: Screen,
        font: &'static Font,
        display_test: bool,
        scanout: Option<crate::scanout::Scanout>,
        pending: alloc::vec::Vec<crate::scanout::Pending>,
    ) -> Self {
        let frequency = tsc_frequency();
        let scanout_period = frequency
            .saturating_mul(SCANOUT_FRAME_MICROS)
            .checked_div(1_000_000)
            .unwrap_or(1)
            .max(1);
        let compositor = Self {
            endpoint,
            screen,
            font,
            windows: [None; MAX_WINDOWS],
            surfaces: [None; MAX_SURFACES],
            next_window_id: 1,
            next_surface_id: 1,
            focus: 0,
            seat: Seat::new(),
            drag: None,
            resize: None,
            driver_input: false,
            display_test,
            scanout,
            pending,
            benchmarked: false,
            benchmark_at: tsc().saturating_add(frequency.saturating_mul(2)),
            resize_retry_at: 0,
            scanout_pending: false,
            scanout_flush_at: 0,
            scanout_period,
            cursor_drawn: None,
        };
        damage_taskbar(
            &compositor.screen,
            &compositor.windows,
            compositor.focus,
            compositor.font,
            compositor.seat.pointer.cursor(),
        );
        compositor
    }

    /// 主循环：提交帧、处理控制台输入并分派服务消息。
    pub(crate) fn run(&mut self) -> ! {
        while !self.pending.is_empty() {
            let message = self.pending.remove(0);
            self.handle_message(
                message.label,
                message.word,
                message.sender,
                message.length,
                message.handle,
                &message.payload[..message.length],
            );
        }
        let mut payload = [0_u8; 8 + TITLE_MAX];
        loop {
            // 成批取空队列：连续指针移动只应用最后位置，整批只合成、提交一次。
            // 指针移动与非指针消息之间保持顺序，避免按键落在过期的指针位置上。
            let mut pointer = None;
            let mut messages = 0;
            loop {
                let (label, word, sender, length, handle) =
                    recv_data_handle_try(self.endpoint, &mut payload, 0);
                if label == 0 {
                    break;
                }
                if label == INPUT_POINTER {
                    pointer = Some(word);
                } else {
                    if let Some(word) = pointer.take() {
                        self.input_pointer(word);
                    }
                    self.handle_message(label, word, sender, length, handle, &payload);
                }
                messages += 1;
                if messages >= BATCH_LIMIT {
                    break;
                }
            }
            if let Some(word) = pointer.take() {
                self.input_pointer(word);
            }
            if self.screen.present() {
                self.scanout_pending = true;
            }
            self.flush_scanout();
            self.benchmark();
            self.retry_window_resizes();
            // 输入驱动接入后不再读内核控制台，避免两路输入重复。
            if !self.driver_input {
                while let Some(byte) = console_read() {
                    self.console_key(byte);
                }
            }
            if messages == 0 {
                let _ = sleep_ticks(1);
            }
        }
    }

    /// 按帧预算提交扫描输出。
    ///
    /// 驱动每次 flush 都要对整帧做 `TRANSFER_TO_HOST_2D` + `RESOURCE_FLUSH`
    /// 并同步等待设备，逐 damage 提交会把 virtio 控制队列和宿主拷贝打满；
    /// 这里把新像素攒到下一帧时间片再提交。
    fn flush_scanout(&mut self) {
        if !self.scanout_pending {
            return;
        }
        let Some(scanout) = &self.scanout else {
            return;
        };
        let now = tsc();
        if now < self.scanout_flush_at {
            return;
        }
        scanout.flush();
        self.scanout_pending = false;
        self.scanout_flush_at = now.saturating_add(self.scanout_period);
    }

    /// 周期补发缩放通知：客户端偶发分配失败时，下一次通知会触发重试，
    /// 避免窗口尺寸已变但内容永久停在旧表面。
    fn retry_window_resizes(&mut self) {
        // 拖动缩放期间由释放事件统一通知，避免边拖边分配表面。
        if self.resize.is_some() {
            return;
        }
        let now = tsc();
        if now < self.resize_retry_at {
            return;
        }
        let period = tsc_frequency()
            .saturating_mul(RESIZE_RETRY_MICROS)
            .checked_div(1_000_000)
            .unwrap_or(1)
            .max(1);
        self.resize_retry_at = now.saturating_add(period);
        for window in self.windows.iter().flatten() {
            if window.width != window.surface.width || window.height != window.surface.height {
                let _ = hk_user_api::send(
                    window.surface.client,
                    DISPLAY_RESIZE,
                    pack_window_size(window.id, window.width as u32, window.height as u32),
                );
            }
        }
    }

    fn handle_message(
        &mut self,
        label: u64,
        word: u64,
        sender: u64,
        length: usize,
        handle: u64,
        payload: &[u8],
    ) {
        match label {
            SURFACE_CREATE => self.create_surface(word, sender, handle),
            WINDOW_CREATE => self.attach_window(word, sender),
            WINDOW_RESIZE => self.replace_window_surface(word, handle, sender),
            SURFACE_DAMAGE => self.mark_surface_damage(payload, length),
            SURFACE_SET_INPUT_REGION => self.set_surface_input_region(payload, length),
            SURFACE_SET_CURSOR => self.set_surface_cursor(word),
            SURFACE_COMMIT => self.commit_surface(word, sender),
            SURFACE_DESTROY => self.destroy_surface(word),
            SEAT_FOCUS => self.focus_window(word),
            SEAT_SET_CLIPBOARD => {
                self.set_clipboard(sender, &payload[..length.min(payload.len())]);
            }
            SEAT_GET_CLIPBOARD => self.send_clipboard(sender),
            INPUT_POINTER => self.input_pointer(word),
            INPUT_BUTTON => self.input_button(word),
            INPUT_KEY => self.input_key(word),
            INPUT_WHEEL => self.input_wheel(word),
            DISPLAY_SET_TITLE => self.set_title_message(payload, length),
            DISPLAY_SHUTDOWN => self.shutdown(),
            DISPLAY_RESTART => {
                write(b"hk-server-display: restart requested\n");
                exit(1)
            }
            _ => {}
        }
    }

    fn set_title_message(&mut self, payload: &[u8], length: usize) {
        let length = length.min(payload.len());
        if length < 8 {
            return;
        }
        let id = u64::from_le_bytes([
            payload[0], payload[1], payload[2], payload[3], payload[4], payload[5], payload[6],
            payload[7],
        ]);
        self.set_window_title(id, &payload[8..length]);
    }

    fn set_clipboard(&mut self, sender: u64, text: &[u8]) {
        if self.seat.set_clipboard(sender, text) {
            Line::new()
                .text(b"hk-server-display: clipboard set ")
                .number(sender)
                .text(b"\n")
                .finish();
        }
    }

    fn send_clipboard(&self, sender: u64) {
        let text = self.seat.clipboard();
        let _ = send_data(sender, SEAT_CLIPBOARD, text);
        Line::new()
            .text(b"hk-server-display: clipboard get ")
            .number(self.seat.clipboard_owner())
            .text(b" ")
            .number(text.len() as u64)
            .text(b"\n")
            .finish();
    }

    fn shutdown(&mut self) -> ! {
        for window in self.windows.iter().flatten() {
            let _ = close_handle(window.surface.handle);
        }
        write(b"hk-server-display: bye\n");
        exit(0)
    }

    fn benchmark(&mut self) {
        if self.benchmarked || tsc() < self.benchmark_at {
            return;
        }
        self.benchmarked = true;
        // 空闲后连续测量，取最小值，排除启动期抢占等待带来的抖动。
        let mut best = u64::MAX;
        for _ in 0..3 {
            best = best.min(self.measure_full_redraw());
        }
        report_full_redraw(b"full redraw min ", best);
    }

    /// 全屏合成并提交，返回耗时 cycles。
    fn measure_full_redraw(&self) -> u64 {
        let start = tsc();
        compose_region(
            &self.screen,
            &self.windows,
            self.focus,
            self.font,
            self.seat.pointer.cursor(),
            0,
            0,
            self.screen.width,
            self.screen.height,
        );
        self.screen.present();
        tsc().saturating_sub(start)
    }
}

/// 输出全屏重绘耗时。
fn report_full_redraw(label: &[u8], cycles: u64) {
    let frequency = tsc_frequency();
    write(b"hk-server-display: ");
    write(label);
    let micros = cycles.saturating_mul(1_000_000).checked_div(frequency);
    write_u64(micros.unwrap_or(cycles));
    write(if micros.is_some() {
        b" us\n".as_slice()
    } else {
        b" cycles\n".as_slice()
    });
}
