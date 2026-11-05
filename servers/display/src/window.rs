//! 窗口模型、屏幕访问与 z 序。

use hk_server_api::display::TITLE_MAX;

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
    height.saturating_sub(TASKBAR_HEIGHT)
}

/// 任务栏按钮左边界。
#[cfg(target_os = "none")]
pub(crate) fn taskbar_button_x(index: usize) -> usize {
    TASKBAR_MARGIN + index * (TASKBAR_BUTTON_WIDTH + TASKBAR_GAP)
}

/// 任务栏按钮上边界。
#[cfg(target_os = "none")]
pub(crate) fn taskbar_button_top(height: usize) -> usize {
    taskbar_top(height) + (TASKBAR_HEIGHT - TASKBAR_BUTTON_HEIGHT) / 2
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
    if y < top || y >= top + TASKBAR_BUTTON_HEIGHT {
        return None;
    }
    windows
        .iter()
        .flatten()
        .enumerate()
        .find_map(|(index, window)| {
            let left = taskbar_button_x(index);
            (left < width && x >= left && x < left + TASKBAR_BUTTON_WIDTH).then_some(window.id)
        })
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
    pub(crate) virt: u64,
    pub(crate) handle: u64,
    pub(crate) client: u64,
    pub(crate) title: [u8; TITLE_MAX],
    pub(crate) title_length: usize,
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
    pub(crate) base: *mut u32,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) stride: usize,
}

#[cfg(target_os = "none")]
impl Screen {
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
}

/// 关闭按钮左上角。
#[cfg(target_os = "none")]
pub(crate) fn close_button_origin(window: &Window) -> (usize, usize) {
    let left = window
        .x
        .saturating_add(window.width)
        .saturating_sub(CLOSE_SIZE + CLOSE_MARGIN + 2);
    let top = window
        .y
        .saturating_add(TITLE_HEIGHT.saturating_sub(CLOSE_SIZE) / 2);
    (left, top)
}

/// 指定点是否命中窗口的关闭按钮。
#[cfg(target_os = "none")]
pub(crate) fn close_at(window: &Window, x: usize, y: usize) -> bool {
    let (left, top) = close_button_origin(window);
    x >= left && x < left + CLOSE_SIZE && y >= top && y < top + CLOSE_SIZE
}

/// 返回指定点最上层窗口。
#[cfg(target_os = "none")]
pub(crate) fn window_at(
    windows: &[Option<Window>; MAX_WINDOWS],
    x: usize,
    y: usize,
) -> Option<Window> {
    windows.iter().rev().flatten().copied().find(|window| {
        x >= window.x
            && x < window.x + window.width
            && y >= window.y
            && y < window.y + window.height + TITLE_HEIGHT
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
