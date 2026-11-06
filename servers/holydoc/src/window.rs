//! Display 窗口：标题、渲染与提交。

use alloc::format;

use hk_server_api::display::{SURFACE_COMMIT, SURFACE_FRAME_DONE, set_title};
use hk_server_api::{recv, send};
use lovyangfx_fonts::Font;

use crate::document::DocView;
use crate::render::render;

/// 提交并等待显示服务完成合成。
pub(crate) fn commit(endpoint: u64, display: u64, surface: u64) {
    let _ = send(display, SURFACE_COMMIT, surface);
    loop {
        let (label, word, _sender) = recv(endpoint);
        if label == SURFACE_FRAME_DONE && word == surface {
            return;
        }
    }
}

/// 刷新标题、渲染并提交。
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply(
    view: &DocView,
    font: &Font<'static>,
    pixels: *mut u32,
    endpoint: u64,
    display: u64,
    surface: u64,
    window: u64,
) {
    let title = format!("HolyDoc: {}", view.title);
    set_title(display, window, &title);
    render(view, font, pixels);
    commit(endpoint, display, surface);
}
