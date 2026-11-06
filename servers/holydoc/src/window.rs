//! Display 窗口：标题、渲染与提交。

use alloc::format;

use hk_server_api::display::{SURFACE_COMMIT, SURFACE_FRAME_DONE, SURFACE_VIRT, set_title};
use hk_server_api::{recv, send};
use lovyangfx_fonts::Font;

use crate::document::{DocView, HEIGHT, WIDTH};
use crate::render::render;

/// 更新窗口标题。
pub(crate) fn update_title(display: u64, window: u64, view: &DocView) {
    let title = format!("HolyDoc: {}", view.title);
    set_title(display, window, &title);
}

/// 把渲染缓冲拷贝到共享表面并提交。
pub(crate) fn present(source: &[u32], endpoint: u64, display: u64, surface: u64) {
    let pixels = SURFACE_VIRT as *mut u32;
    // SAFETY: 渲染缓冲与表面帧都已映射，长度均为 WIDTH * HEIGHT。
    unsafe {
        core::ptr::copy_nonoverlapping(source.as_ptr(), pixels, WIDTH * HEIGHT);
    }
    commit(endpoint, display, surface);
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
    font: &Font<'static>,
    pixels: *mut u32,
    endpoint: u64,
    display: u64,
    surface: u64,
    window: u64,
) {
    update_title(display, window, view);
    render(view, font, pixels);
    commit(endpoint, display, surface);
}
