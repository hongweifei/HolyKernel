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
    pub(crate) taskbar_bg: u32,
    pub(crate) taskbar_button: u32,
    pub(crate) taskbar_active: u32,
    pub(crate) taskbar_text: u32,
    pub(crate) cursor: u32,
    pub(crate) cursor_shadow: u32,
}

/// 深色主题。
static DARK: Theme = Theme {
    desktop_top: 0x0010_1820,
    desktop_bottom: 0x0030_4860,
    title_color: 0x0020_60A0,
    title_focus: 0x0040_A0E0,
    title_text: 0x00E0_F0FF,
    title_close: 0x00E8_6868,
    taskbar_bg: 0x0018_1E28,
    taskbar_button: 0x0028_3240,
    taskbar_active: 0x0038_6A98,
    taskbar_text: 0x00E8_F0FF,
    cursor: 0x00FF_FFFF,
    cursor_shadow: 0x0000_0000,
};

/// 浅色主题。
static LIGHT: Theme = Theme {
    desktop_top: 0x00E8_EEF5,
    desktop_bottom: 0x00C8_D8E8,
    title_color: 0x0080_A8C8,
    title_focus: 0x0030_78C0,
    title_text: 0x00FF_FFFF,
    title_close: 0x00C0_3030,
    taskbar_bg: 0x00D8_E0EA,
    taskbar_button: 0x00C0_CCDA,
    taskbar_active: 0x0080_B0E0,
    taskbar_text: 0x0020_2830,
    cursor: 0x0000_0000,
    cursor_shadow: 0x00FF_FFFF,
};

static CURRENT: AtomicUsize = AtomicUsize::new(0);

/// 按启动命令行选择主题，返回主题名称。
pub(crate) fn select(cmdline: Option<&str>) -> &'static str {
    if cmdline.is_some_and(|line| line.contains("theme=light")) {
        CURRENT.store(1, Ordering::Relaxed);
        "light"
    } else {
        CURRENT.store(0, Ordering::Relaxed);
        "dark"
    }
}

/// 返回当前主题。
pub(crate) fn theme() -> &'static Theme {
    if CURRENT.load(Ordering::Relaxed) == 1 {
        &LIGHT
    } else {
        &DARK
    }
}
