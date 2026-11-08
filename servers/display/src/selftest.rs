//! Display 自检：标题栏像素与焦点配色。

use crate::theme;
use crate::window::{MAX_WINDOWS, Window, title_height};

/// 标题栏是否已经画出与背景不同的像素。
pub(crate) fn title_visible(
    screen: &crate::window::Screen,
    windows: &[Option<Window>; MAX_WINDOWS],
    focus: u64,
    id: u64,
) -> bool {
    let Some(window) = windows.iter().flatten().find(|window| window.id == id) else {
        return false;
    };
    let palette = theme::theme();
    let color = if window.id == focus {
        palette.title_focus
    } else {
        palette.title_color
    };
    (window.y + 4..(window.y + title_height()).min(screen.height)).any(|row| {
        (window.x + 4..(window.x + window.width).min(screen.width))
            .any(|column| screen.get(column, row) != color)
    })
}
