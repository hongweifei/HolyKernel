//! Display 窗口：标题、渲染与提交。

use alloc::format;

use hk_canvas::Surface;
use hk_proto::display::{SURFACE_COMMIT, SURFACE_FRAME_DONE, set_title};
use hk_server_api::{recv, send};

use crate::document::DocView;
use crate::render::render;

/// 更新窗口标题。
pub(crate) fn update_title(display: u64, window: u64, view: &DocView) {
    let title = format!("HolyDoc: {}", view.title);
    set_title(display, window, &title);
}

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
    pixels: *mut u32,
    endpoint: u64,
    display: u64,
    surface: u64,
    window: u64,
) {
    update_title(display, window, view);
    // SAFETY: 调用方传入的表面帧已映射，长度为 view.width * view.height。
    let frame = unsafe { Surface::from_raw(pixels, view.width, view.height, view.width) };
    render(view, &frame);
    commit(endpoint, display, surface);
}
