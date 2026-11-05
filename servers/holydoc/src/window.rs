//! Display 窗口：标题、渲染与提交。

use alloc::format;

use hk_server_api::display::{DISPLAY_COMMIT, DISPLAY_FRAME_DONE, set_title};
use hk_server_api::{recv, send};
use lovyangfx_fonts::Font;

use crate::document::DocView;
use crate::render::render;

/// 提交并等待显示服务完成合成。
pub(crate) fn commit(endpoint: u64, display: u64, window: u64) {
    let _ = send(display, DISPLAY_COMMIT, window);
    loop {
        let (label, word, _sender) = recv(endpoint);
        if label == DISPLAY_FRAME_DONE && word == window {
            return;
        }
    }
}

/// 刷新标题、渲染并提交。
pub(crate) fn apply(
    view: &DocView,
    font: &Font<'static>,
    pixels: *mut u32,
    endpoint: u64,
    display: u64,
    window: u64,
) {
    let title = format!("HolyDoc: {}", view.title);
    set_title(display, window, &title);
    render(view, font, pixels);
    commit(endpoint, display, window);
}
