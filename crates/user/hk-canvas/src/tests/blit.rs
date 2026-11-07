//! 表面拷贝与缩放测试。

use crate::Rect;

use super::canvas;

#[test]
fn blit_copies_region() {
    let mut source_pixels = [1_u32, 2, 3, 4];
    let source = canvas(&mut source_pixels, 2, 2);
    let mut target_pixels = [0_u32; 16];
    let surface = canvas(&mut target_pixels, 4, 4);
    surface.blit(1, 1, &source);
    assert_eq!(target_pixels[5], 1);
    assert_eq!(target_pixels[6], 2);
    assert_eq!(target_pixels[9], 3);
    assert_eq!(target_pixels[10], 4);
    assert_eq!(target_pixels[4], 0);
}

#[test]
fn blit_respects_both_clips() {
    let mut source_pixels = [1_u32, 2, 3, 4];
    let source = canvas(&mut source_pixels, 2, 2);
    let mut target_pixels = [0_u32; 16];
    let surface = canvas(&mut target_pixels, 4, 4);
    let window = surface.clipped(Rect::new(2, 2, 2, 2));
    window.blit(1, 1, &source);
    assert_eq!(target_pixels[10], 4, "只有落在裁剪内的像素被拷贝");
    assert_eq!(target_pixels[5], 0);
}

#[test]
fn blit_copies_full_width_rows_contiguously() {
    let mut source_pixels = [1_u32, 2, 3, 4];
    let source = canvas(&mut source_pixels, 2, 2);
    let mut target_pixels = [0_u32; 8];
    let surface = canvas(&mut target_pixels, 2, 4);
    surface.blit(0, 2, &source);
    assert_eq!(target_pixels, [0, 0, 0, 0, 1, 2, 3, 4]);
}

#[test]
fn blit_scaled_upscales_nearest_neighbor() {
    let mut source_pixels = [0xFF_FF_FF_u32];
    let source = canvas(&mut source_pixels, 1, 1);
    let mut target_pixels = [0_u32; 16];
    let surface = canvas(&mut target_pixels, 4, 4);
    surface.blit_scaled(0, 0, 2, 2, &source);
    assert_eq!(target_pixels[0], 0xFF_FFFF);
    assert_eq!(target_pixels[1], 0xFF_FFFF);
    assert_eq!(target_pixels[4], 0xFF_FFFF);
    assert_eq!(target_pixels[5], 0xFF_FFFF);
    assert_eq!(target_pixels[2], 0);
    assert_eq!(target_pixels[8], 0);
}

#[test]
fn blit_scaled_respects_clip() {
    let mut source_pixels = [7_u32, 8];
    let source = canvas(&mut source_pixels, 2, 1);
    let mut target_pixels = [0_u32; 9];
    let surface = canvas(&mut target_pixels, 3, 3);
    let window = surface.clipped(Rect::new(1, 1, 1, 1));
    window.blit_scaled(0, 0, 3, 3, &source);
    assert_eq!(target_pixels[4], 7);
    assert_eq!(target_pixels[0], 0);
}

#[test]
fn blit_with_alpha_blends_source() {
    let mut source_pixels = [0xFF_FF_FF_u32];
    let source = canvas(&mut source_pixels, 1, 1);
    let mut target_pixels = [0_u32; 1];
    let surface = canvas(&mut target_pixels, 1, 1);
    surface.blit_with_alpha(0, 0, &source, 128);
    assert_eq!(target_pixels[0], 0x80_8080);
}

#[test]
fn blit_with_alpha_extremes() {
    let mut source_pixels = [0xFF_FF_FF_u32];
    let source = canvas(&mut source_pixels, 1, 1);
    let mut target_pixels = [0x11_1111_u32; 2];
    let surface = canvas(&mut target_pixels, 2, 1);
    surface.blit_with_alpha(0, 0, &source, 0);
    surface.blit_with_alpha(1, 0, &source, 255);
    assert_eq!(target_pixels, [0x11_1111, 0xFF_FFFF]);
}
