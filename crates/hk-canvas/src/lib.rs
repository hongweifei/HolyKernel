//! 像素绘制：表面、裁剪、几何图元、混合与文本。
//!
//! 绘制目标是一块 `u32` 像素缓冲区，颜色为 `0x00RRGGBB`。表面只描述这块内存的
//! 形状与行跨度，不持有内存，也不做同步；生命周期与独占访问由调用方保证。
//!
//! 裁剪通过 [`Surface::clipped`] 派生视图实现：视图携带自身的裁剪矩形，所有
//! 绘制原语在裁剪内取交集，调用方不需要在每次绘制前判断坐标。
//!
//! 文本既可以用 `hk_font::Font` 显式绘制，也可以走 `draw_text_default`、
//! `draw_glyph_default` 与 `text_width_default`，直接使用 `hk-font` 的内置默认字体。
//!
//! - `rect`：整数矩形与集合运算。
//! - `color`：按比例混合两个颜色。
//! - `surface`：像素读写、填充、混合、裁剪视图、行拷贝与滚动。
//! - `surface::shapes`：圆角矩形、椭圆、圆、线段与渐变。
//! - `surface::blit`：整块拷贝、最近邻缩放与半透明拷贝。
//! - `text`：字形覆盖率位图与字符串落位。

#![no_std]

#[cfg(test)]
extern crate std;

mod color;
mod rect;
mod surface;
#[cfg(test)]
mod tests;
mod text;

pub use color::mix;
pub use rect::Rect;
pub use surface::Surface;
pub use text::{GlyphMask, text_width, text_width_default};
