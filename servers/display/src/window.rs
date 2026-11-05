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
