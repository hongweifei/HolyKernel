//! 内置字体数据：可缩放 TrueType 与位图回退。

use lovyangfx_fonts::Font as BitmapFont;
use lovyangfx_fonts::fonts::efont_cn_16;

/// 内置可缩放字体。
#[cfg(feature = "scalable")]
pub(crate) const FONT_DATA: &[u8] = include_bytes!("../assets/AdwaitaSans-Regular.ttf");

/// 返回内置位图回退字体；ASCII 半宽、汉字全宽。
pub(crate) fn fallback_font() -> BitmapFont<'static> {
    efont_cn_16()
}
