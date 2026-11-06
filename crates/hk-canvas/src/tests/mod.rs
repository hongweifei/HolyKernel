//! 像素原语、形状、拷贝与文本绘制的单元测试。

use crate::{GlyphMask, Surface};

mod blit;
mod color;
mod rect;
mod shapes;
mod surface;
mod text;

/// 从像素缓冲构造整块表面。
fn canvas(pixels: &mut [u32], width: usize, height: usize) -> Surface<'_> {
    Surface::from_slice(pixels, width, height)
}

/// 以覆盖率数据描述一个位图。
fn mask<'a>(coverage: &'a [u8], width: usize, height: usize, left: i32, top: i32) -> GlyphMask<'a> {
    GlyphMask {
        width,
        height,
        left,
        top,
        stride: width,
        coverage,
    }
}
