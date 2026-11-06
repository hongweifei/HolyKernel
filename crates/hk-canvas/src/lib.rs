//! 像素绘制原语：表面、裁剪、填充、混合与文本。
//!
//! 绘制目标是一块 `u32` 像素缓冲区，颜色为 `0x00RRGGBB`。表面只描述这块内存的
//! 形状与行跨度，不持有内存，也不做同步；生命周期与独占访问由调用方保证。
//!
//! 裁剪通过 [`Surface::clipped`] 派生视图实现：视图携带自身的裁剪矩形，所有
//! 绘制原语在裁剪内取交集，调用方不需要在每次绘制前判断坐标。

#![no_std]

#[cfg(test)]
extern crate std;

mod surface;
#[cfg(test)]
mod tests;
mod text;

pub use surface::{Rect, Surface, mix};
pub use text::{GlyphMask, GlyphSource, text_width};
