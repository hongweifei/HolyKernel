//! Display 调色板：深色与浅色主题。
//!
//! 主题属于 Display 服务内部状态，由启动命令行 `theme=light` 选择；窗口、
//! 任务栏与光标统一从当前调色板取色。

use core::sync::atomic::{AtomicUsize, Ordering};

/// 一套界面颜色。
pub(crate) struct Theme {
    pub(crate) desktop_top: u32,
    pub(crate) desktop_bottom: u32,
    pub(crate) title_color: u32,
    pub(crate) title_focus: u32,
    pub(crate) title_text: u32,
    pub(crate) title_close: u32,
    pub(crate) title_button_hover: u32,
    pub(crate) title_close_hover: u32,
    pub(crate) taskbar_bg: u32,
    pub(crate) taskbar_button: u32,
    pub(crate) taskbar_button_hover: u32,
    pub(crate) taskbar_active: u32,
    pub(crate) taskbar_text: u32,
    pub(crate) window_bg: u32,
    pub(crate) cursor: u32,
    pub(crate) cursor_shadow: u32,
    pub(crate) resize_handle: u32,
    pub(crate) shadow: u32,
}

/// 深色主题。
static DARK: Theme = Theme {
    desktop_top: 0x0010_1820,
    desktop_bottom: 0x0030_4860,
    title_color: 0x0020_60A0,
    title_focus: 0x0040_A0E0,
    title_text: 0x00E0_F0FF,
    title_close: 0x00E8_6868,
    title_button_hover: 0x0038_4A5C,
    title_close_hover: 0x00B8_3838,
    taskbar_bg: 0x0018_1E28,
    taskbar_button: 0x0028_3240,
    taskbar_button_hover: 0x0038_4A5C,
    taskbar_active: 0x0038_6A98,
    taskbar_text: 0x00E8_F0FF,
    window_bg: 0x0014_1A20,
    cursor: 0x00FF_FFFF,
    cursor_shadow: 0x0000_0000,
    resize_handle: 0x0060_90C0,
    shadow: 0x0000_0000,
};

/// 浅色主题。
static LIGHT: Theme = Theme {
    desktop_top: 0x00E8_EEF5,
    desktop_bottom: 0x00C8_D8E8,
    title_color: 0x0080_A8C8,
    title_focus: 0x0030_78C0,
    title_text: 0x00FF_FFFF,
    title_close: 0x00C0_3030,
    title_button_hover: 0x0068_90B8,
    title_close_hover: 0x00E0_5050,
    taskbar_bg: 0x00D8_E0EA,
    taskbar_button: 0x00C0_CCDA,
    taskbar_button_hover: 0x00A8_BCCE,
    taskbar_active: 0x0080_B0E0,
    taskbar_text: 0x0020_2830,
    window_bg: 0x00F4_F6FA,
    cursor: 0x0000_0000,
    cursor_shadow: 0x00FF_FFFF,
    resize_handle: 0x0040_6090,
    shadow: 0x0030_4050,
};

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static SCALE: AtomicUsize = AtomicUsize::new(1);

/// 按启动命令行选择主题与界面缩放，返回主题名称。
pub(crate) fn select(cmdline: Option<&str>) -> &'static str {
    let line = cmdline.unwrap_or("");
    let mut scale = 1;
    for part in line.split_whitespace() {
        if let Some(value) = part.strip_prefix("ui.scale=") {
            scale = value.parse().unwrap_or(1).clamp(1, 2);
        }
    }
    SCALE.store(scale, Ordering::Relaxed);
    if line.contains("theme=light") {
        CURRENT.store(1, Ordering::Relaxed);
        "light"
    } else {
        CURRENT.store(0, Ordering::Relaxed);
        "dark"
    }
}

/// 界面缩放倍率。
pub(crate) fn scale() -> usize {
    SCALE.load(Ordering::Relaxed).clamp(1, 2)
}

/// 返回当前主题。
pub(crate) fn theme() -> &'static Theme {
    if CURRENT.load(Ordering::Relaxed) == 1 {
        &LIGHT
    } else {
        &DARK
    }
}
