//! Display 窗口：标题、渲染与提交。

use alloc::format;

use hk_canvas::Surface;
use hk_proto::display::Window;

use crate::document::DocView;
use crate::render::render;

/// 更新窗口标题。
pub(crate) fn update_title(window: &Window, view: &DocView) {
    let title = format!("HolyDoc: {}", view.title);
    window.set_title(&title);
}

/// 刷新标题、渲染并提交。
pub(crate) fn apply(view: &DocView, window: &Window) {
    update_title(window, view);
    let (width, height) = (window.width(), window.height());
    // SAFETY: 调用方传入的表面帧已映射，长度为 width * height。
    let frame = unsafe { Surface::from_raw(window.pixels(), width, height, width) };
    render(view, &frame);
    window.commit();
}
