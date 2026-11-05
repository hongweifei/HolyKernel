//! 窗口模型、屏幕访问与 z 序。

use core::cell::Cell;

use hk_server_api::display::TITLE_MAX;

use crate::theme::scale;

/// 最多同时合成的窗口数。
#[cfg(target_os = "none")]
pub(crate) const MAX_WINDOWS: usize = 8;

/// 窗口表面映射基址。
#[cfg(target_os = "none")]
pub(crate) const WINDOW_BASE: u64 = 0x6000_0000;

/// 相邻窗口表面的地址间隔。
#[cfg(target_os = "none")]
pub(crate) const WINDOW_STRIDE: u64 = 0x20_0000;

/// 标题栏高度。
#[cfg(target_os = "none")]
pub(crate) const TITLE_HEIGHT: usize = 24;

/// 窗口最小宽度。
#[cfg(target_os = "none")]
pub(crate) const MIN_WINDOW_WIDTH: usize = 120;

/// 窗口最小高度。
#[cfg(target_os = "none")]
pub(crate) const MIN_WINDOW_HEIGHT: usize = 80;

/// 缩放把手尺寸。
#[cfg(target_os = "none")]
pub(crate) const RESIZE_SIZE: usize = 12;

/// 标题栏按钮间距。
#[cfg(target_os = "none")]
pub(crate) const TITLE_BUTTON_GAP: usize = 4;

/// 标题栏关闭按钮尺寸。
#[cfg(target_os = "none")]
pub(crate) const CLOSE_SIZE: usize = 10;

/// 标题栏关闭按钮右边距。
#[cfg(target_os = "none")]
pub(crate) const CLOSE_MARGIN: usize = 4;

/// 底部任务栏高度。
#[cfg(target_os = "none")]
pub(crate) const TASKBAR_HEIGHT: usize = 28;

/// 任务栏左右留白。
#[cfg(target_os = "none")]
pub(crate) const TASKBAR_MARGIN: usize = 8;

/// 任务栏按钮宽度。
#[cfg(target_os = "none")]
pub(crate) const TASKBAR_BUTTON_WIDTH: usize = 160;

/// 任务栏按钮高度。
#[cfg(target_os = "none")]
pub(crate) const TASKBAR_BUTTON_HEIGHT: usize = 20;

/// 任务栏按钮间距。
#[cfg(target_os = "none")]
pub(crate) const TASKBAR_GAP: usize = 6;

/// 任务栏顶边。
#[cfg(target_os = "none")]
pub(crate) fn taskbar_top(height: usize) -> usize {
    height.saturating_sub(taskbar_height())
}

/// 任务栏按钮左边界。
#[cfg(target_os = "none")]
pub(crate) fn taskbar_button_x(index: usize) -> usize {
    taskbar_margin() + index * (taskbar_button_width() + taskbar_gap())
}

/// 任务栏按钮上边界。
#[cfg(target_os = "none")]
pub(crate) fn taskbar_button_top(height: usize) -> usize {
    taskbar_top(height) + (taskbar_height() - taskbar_button_height()) / 2
}

/// 返回任务栏按钮对应的窗口编号。
#[cfg(target_os = "none")]
pub(crate) fn taskbar_at(
    windows: &[Option<Window>; MAX_WINDOWS],
    x: usize,
    y: usize,
    width: usize,
    height: usize,
) -> Option<u64> {
    let top = taskbar_button_top(height);
    if y < top || y >= top + taskbar_button_height() {
        return None;
    }
    windows
        .iter()
        .flatten()
        .enumerate()
        .find_map(|(index, window)| {
            let left = taskbar_button_x(index);
            (left < width && x >= left && x < left + taskbar_button_width()).then_some(window.id)
        })
}

/// 标题栏高度（含缩放）。
#[cfg(target_os = "none")]
pub(crate) fn title_height() -> usize {
    TITLE_HEIGHT * scale()
}

/// 任务栏高度（含缩放）。
#[cfg(target_os = "none")]
pub(crate) fn taskbar_height() -> usize {
    TASKBAR_HEIGHT * scale()
}

/// 任务栏留白（含缩放）。
#[cfg(target_os = "none")]
pub(crate) fn taskbar_margin() -> usize {
    TASKBAR_MARGIN * scale()
}

/// 任务栏按钮宽度（含缩放）。
#[cfg(target_os = "none")]
pub(crate) fn taskbar_button_width() -> usize {
    TASKBAR_BUTTON_WIDTH * scale()
}

/// 任务栏按钮高度（含缩放）。
#[cfg(target_os = "none")]
pub(crate) fn taskbar_button_height() -> usize {
    TASKBAR_BUTTON_HEIGHT * scale()
}

/// 任务栏按钮间距（含缩放）。
#[cfg(target_os = "none")]
pub(crate) fn taskbar_gap() -> usize {
    TASKBAR_GAP * scale()
}

/// 标题栏按钮尺寸（含缩放）。
#[cfg(target_os = "none")]
pub(crate) fn close_size() -> usize {
    CLOSE_SIZE * scale()
}

/// 标题栏按钮右边距（含缩放）。
#[cfg(target_os = "none")]
pub(crate) fn close_margin() -> usize {
    CLOSE_MARGIN * scale()
}

/// 标题栏按钮间距（含缩放）。
#[cfg(target_os = "none")]
pub(crate) fn title_gap() -> usize {
    TITLE_BUTTON_GAP * scale()
}

/// 缩放把手尺寸（含缩放）。
#[cfg(target_os = "none")]
pub(crate) fn resize_size() -> usize {
    RESIZE_SIZE * scale()
}

/// 已创建的窗口；槽位顺序即 z 序（越靠后越靠上）。
#[cfg(target_os = "none")]
#[derive(Clone, Copy)]
pub(crate) struct Window {
    pub(crate) id: u64,
    pub(crate) x: usize,
    pub(crate) y: usize,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) surface_width: usize,
    pub(crate) surface_height: usize,
    pub(crate) virt: u64,
    pub(crate) handle: u64,
    pub(crate) client: u64,
    pub(crate) title: [u8; TITLE_MAX],
    pub(crate) title_length: usize,
    pub(crate) restore: Option<(usize, usize, usize, usize)>,
    pub(crate) minimized: bool,
}

/// 正在缩放的窗口。
#[cfg(target_os = "none")]
#[derive(Clone, Copy)]
pub(crate) struct Resize {
    pub(crate) id: u64,
    pub(crate) start_x: usize,
    pub(crate) start_y: usize,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) resized: bool,
}

/// 正在拖动的窗口。
#[cfg(target_os = "none")]
#[derive(Clone, Copy)]
pub(crate) struct Drag {
    pub(crate) id: u64,
    pub(crate) offset_x: usize,
    pub(crate) offset_y: usize,
    pub(crate) moved: bool,
}

#[cfg(target_os = "none")]
pub(crate) struct Screen {
    base: *mut u32,
    fb: *mut u32,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) stride: usize,
    damage: Cell<Option<(usize, usize, usize, usize)>>,
}

#[cfg(target_os = "none")]
impl Screen {
    pub(crate) fn new(
        back: *mut u32,
        fb: *mut u32,
        width: usize,
        height: usize,
        stride: usize,
    ) -> Self {
        Self {
            base: back,
            fb,
            width,
            height,
            stride,
            damage: Cell::new(None),
        }
    }

    /// 记录需要从后备缓冲提交到帧缓冲的区域。
    pub(crate) fn mark_damage(&self, x: usize, y: usize, width: usize, height: usize) {
        let left = x.min(self.width);
        let top = y.min(self.height);
        let right = x.saturating_add(width).min(self.width);
        let bottom = y.saturating_add(height).min(self.height);
        if left >= right || top >= bottom {
            return;
        }
        let union = match self.damage.get() {
            Some((old_left, old_top, old_right, old_bottom)) => (
                old_left.min(left),
                old_top.min(top),
                old_right.max(right),
                old_bottom.max(bottom),
            ),
            None => (left, top, right, bottom),
        };
        self.damage.set(Some(union));
    }

    /// 把累积的 damage 一次性提交到帧缓冲。
    pub(crate) fn present(&self) {
        let Some((left, top, right, bottom)) = self.damage.take() else {
            return;
        };
        let count = right - left;
        for row in top..bottom {
            // SAFETY: 后备缓冲与帧缓冲均为 width×height，区域已裁剪；VGA 内存逐元素写。
            unsafe {
                let source = self.base.add(row * self.stride + left);
                let destination = self.fb.add(row * self.stride + left);
                for index in 0..count {
                    destination
                        .add(index)
                        .write_volatile(source.add(index).read());
                }
            }
        }
    }

    pub(crate) fn put(&self, x: usize, y: usize, color: u32) {
        if x >= self.width || y >= self.height {
            return;
        }
        // SAFETY: 坐标已裁剪到帧缓冲范围内。
        unsafe {
            self.base.add(y * self.stride + x).write_volatile(color);
        }
    }

    pub(crate) fn get(&self, x: usize, y: usize) -> u32 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        // SAFETY: 坐标已裁剪到帧缓冲范围内。
        unsafe { self.base.add(y * self.stride + x).read_volatile() }
    }

    /// 用单色填充一行。
    pub(crate) fn fill_row(&self, x: usize, y: usize, width: usize, color: u32) {
        if y >= self.height || width == 0 {
            return;
        }
        let left = x.min(self.width);
        let right = x.saturating_add(width).min(self.width);
        if left >= right {
            return;
        }
        // SAFETY: 行范围已裁剪到帧缓冲内；批量写入避免逐像素 volatile。
        unsafe {
            let row = core::slice::from_raw_parts_mut(
                self.base.add(y * self.stride + left),
                right - left,
            );
            row.fill(color);
        }
    }

    /// 从客户端表面整行拷贝到帧缓冲。
    pub(crate) fn copy_row(&self, x: usize, y: usize, count: usize, source: *const u32) {
        if y >= self.height || count == 0 {
            return;
        }
        let left = x.min(self.width);
        let count = count.min(self.width - left);
        if count == 0 {
            return;
        }
        // SAFETY: 源行由调用方按表面尺寸裁剪；目标行已裁剪到帧缓冲。
        unsafe {
            core::ptr::copy_nonoverlapping(source, self.base.add(y * self.stride + left), count);
        }
    }
}

/// 关闭按钮左上角。
#[cfg(target_os = "none")]
pub(crate) fn close_button_origin(window: &Window) -> (usize, usize) {
    let left = window
        .x
        .saturating_add(window.width)
        .saturating_sub(close_size() + close_margin() + 2 * scale());
    let top = window
        .y
        .saturating_add(title_height().saturating_sub(close_size()) / 2);
    (left, top)
}

/// 最大化/还原按钮左上角。
#[cfg(target_os = "none")]
pub(crate) fn maximize_button_origin(window: &Window) -> (usize, usize) {
    let (close_x, top) = close_button_origin(window);
    (close_x.saturating_sub(CLOSE_SIZE + TITLE_BUTTON_GAP), top)
}

/// 最小化按钮左上角。
#[cfg(target_os = "none")]
pub(crate) fn minimize_button_origin(window: &Window) -> (usize, usize) {
    let (maximize_x, top) = maximize_button_origin(window);
    (maximize_x.saturating_sub(close_size() + title_gap()), top)
}

/// 指定点是否命中窗口的关闭按钮。
#[cfg(target_os = "none")]
pub(crate) fn close_at(window: &Window, x: usize, y: usize) -> bool {
    let (left, top) = close_button_origin(window);
    x >= left && x < left + close_size() && y >= top && y < top + close_size()
}

/// 指定点是否命中最大化/还原按钮。
#[cfg(target_os = "none")]
pub(crate) fn maximize_at(window: &Window, x: usize, y: usize) -> bool {
    let (left, top) = maximize_button_origin(window);
    x >= left && x < left + close_size() && y >= top && y < top + close_size()
}

/// 指定点是否命中最小化按钮。
#[cfg(target_os = "none")]
pub(crate) fn minimize_at(window: &Window, x: usize, y: usize) -> bool {
    let (left, top) = minimize_button_origin(window);
    x >= left && x < left + close_size() && y >= top && y < top + close_size()
}

/// 指定点是否命中窗口右下角的缩放把手。
#[cfg(target_os = "none")]
pub(crate) fn resize_at(window: &Window, x: usize, y: usize) -> bool {
    let right = window.x.saturating_add(window.width);
    let bottom = window
        .y
        .saturating_add(title_height())
        .saturating_add(window.height);
    x >= right.saturating_sub(resize_size())
        && x < right
        && y >= bottom.saturating_sub(resize_size())
        && y < bottom
}

/// 返回指定点最上层窗口。
#[cfg(target_os = "none")]
pub(crate) fn window_at(
    windows: &[Option<Window>; MAX_WINDOWS],
    x: usize,
    y: usize,
) -> Option<Window> {
    windows
        .iter()
        .rev()
        .flatten()
        .filter(|window| !window.minimized)
        .copied()
        .find(|window| {
            x >= window.x
                && x < window.x + window.width
                && y >= window.y
                && y < window.y + window.height + title_height()
        })
}

/// 把窗口提升到 z 序顶部。
#[cfg(target_os = "none")]
pub(crate) fn raise(windows: &mut [Option<Window>; MAX_WINDOWS], id: u64) {
    let Some(position) = windows
        .iter()
        .position(|window| window.as_ref().is_some_and(|window| window.id == id))
    else {
        return;
    };
    let Some(last) = windows.iter().rposition(Option::is_some) else {
        return;
    };
    if position < last {
        windows[position..=last].rotate_left(1);
    }
}
