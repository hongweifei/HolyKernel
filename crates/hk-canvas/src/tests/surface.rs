//! 表面填充、混合、裁剪、拷贝与滚动测试。

use crate::{Rect, Surface};

use super::canvas;

#[test]
fn fill_stays_inside_surface() {
    let mut pixels = [0_u32; 12];
    let surface = canvas(&mut pixels, 4, 3);
    surface.fill(Rect::new(-2, -1, 10, 10), 0x11_2233);
    assert!(pixels.iter().all(|pixel| *pixel == 0x11_2233));
}

#[test]
fn fill_ignores_empty_rect() {
    let mut pixels = [7_u32; 12];
    let surface = canvas(&mut pixels, 4, 3);
    surface.fill(Rect::new(1, 1, 0, 2), 0);
    surface.fill(Rect::new(1, 1, 2, -1), 0);
    assert!(pixels.iter().all(|pixel| *pixel == 7));
}

#[test]
fn clear_and_fill_row_respect_clip() {
    let mut pixels = [0_u32; 12];
    let surface = canvas(&mut pixels, 4, 3);
    let view = surface.clipped(Rect::new(1, 0, 2, 2));
    view.clear(9);
    view.fill_row(0, 1, 4, 4);
    assert_eq!(pixels[5], 4);
    assert_eq!(pixels[6], 4);
    assert_eq!(pixels[1], 9);
    assert_eq!(pixels[0], 0);
}

#[test]
fn clip_bounds_every_primitive() {
    let mut pixels = [0_u32; 16];
    let surface = canvas(&mut pixels, 4, 4);
    let window = surface.clipped(Rect::new(1, 1, 2, 2));
    window.fill(Rect::new(0, 0, 4, 4), 0xFF_FFFF);
    window.put(0, 0, 0);
    window.put(3, 3, 0);
    assert_eq!(window.get(0, 0), None);
    assert_eq!(surface.get(0, 0), Some(0));
    let painted = [5, 6, 9, 10];
    for (index, pixel) in pixels.iter().enumerate() {
        let expected = if painted.contains(&index) {
            0xFF_FFFF
        } else {
            0
        };
        assert_eq!(*pixel, expected, "像素 {index} 越界绘制");
    }
}

#[test]
fn clipped_view_does_not_disturb_parent() {
    let mut pixels = [0_u32; 16];
    let surface = canvas(&mut pixels, 4, 4);
    let inner = surface.clipped(Rect::new(1, 1, 2, 2));
    assert_eq!(surface.clip(), Rect::new(0, 0, 4, 4));
    assert_eq!(inner.clip(), Rect::new(1, 1, 2, 2));
    assert_eq!(
        inner.clipped(Rect::new(2, 2, 8, 8)).clip(),
        Rect::new(2, 2, 1, 1)
    );
}

#[test]
fn blend_reads_and_writes_the_same_pixel() {
    let mut pixels = [0x00_00_00_u32; 4];
    let surface = canvas(&mut pixels, 2, 2);
    surface.blend(1, 1, 0xFF_FF_FF, 51);
    surface.blend(5, 5, 0xFF_FF_FF, 255);
    assert_eq!(pixels[3], 0x33_33_33);
    assert_eq!(pixels[0], 0);
}

#[test]
fn blend_ratio_extremes_skip_and_replace() {
    let mut pixels = [0x11_11_11_u32; 2];
    let surface = canvas(&mut pixels, 2, 1);
    surface.blend(0, 0, 0xFF_FF_FF, 0);
    surface.blend(1, 0, 0xFF_FF_FF, 255);
    assert_eq!(pixels, [0x11_11_11, 0xFF_FF_FF]);
}

#[test]
fn blend_rect_covers_region() {
    let mut pixels = [0x00_00_00_u32; 9];
    let surface = canvas(&mut pixels, 3, 3);
    surface.blend_rect(Rect::new(1, 0, 2, 1), 0xFF_FF_FF, 255);
    assert_eq!(pixels[..3], [0, 0xFF_FF_FF, 0xFF_FF_FF]);
    assert_eq!(pixels[3..], [0; 6]);
}

#[test]
fn blend_rect_ratio_zero_is_noop() {
    let mut pixels = [5_u32; 9];
    let surface = canvas(&mut pixels, 3, 3);
    surface.blend_rect(Rect::new(0, 0, 3, 3), 0xFF_FF_FF, 0);
    assert_eq!(pixels, [5; 9]);
}

#[test]
fn copy_row_clips_to_intersection() {
    let mut pixels = [0_u32; 6];
    let source = [1_u32, 2, 3, 4];
    let surface = canvas(&mut pixels, 3, 2);
    let view = surface.clipped(Rect::new(1, 0, 2, 2));
    // SAFETY: 源行长度为 4，覆盖整个拷贝区间。
    unsafe { view.copy_row(0, 1, 4, source.as_ptr()) };
    assert_eq!(pixels[3..], [0, 2, 3]);
}

#[test]
fn copy_row_outside_clip_is_noop() {
    let mut pixels = [0_u32; 4];
    let surface = canvas(&mut pixels, 2, 2);
    // SAFETY: 源行长度为 2，未越界。
    unsafe { surface.copy_row(0, 5, 2, [9_u32, 9].as_ptr()) };
    assert_eq!(pixels, [0; 4]);
}

#[test]
fn scroll_moves_content_right() {
    let mut pixels = [1_u32, 2, 3, 4];
    let surface = canvas(&mut pixels, 4, 1);
    surface.scroll(Rect::new(0, 0, 4, 1), 1, 0);
    assert_eq!(pixels, [1, 1, 2, 3]);
}

#[test]
fn scroll_moves_content_left() {
    let mut pixels = [1_u32, 2, 3, 4];
    let surface = canvas(&mut pixels, 4, 1);
    surface.scroll(Rect::new(0, 0, 4, 1), -1, 0);
    assert_eq!(pixels, [2, 3, 4, 4]);
}

#[test]
fn scroll_moves_content_down_without_corruption() {
    let mut pixels = [1_u32, 2, 3, 4, 5, 6];
    let surface = canvas(&mut pixels, 3, 2);
    surface.scroll(Rect::new(0, 0, 3, 2), 0, 1);
    assert_eq!(pixels, [1, 2, 3, 1, 2, 3]);
}

#[test]
fn scroll_only_updates_the_destination_region() {
    let mut pixels = [1_u32, 2, 3, 4, 5, 6];
    let surface = canvas(&mut pixels, 3, 2);
    surface.scroll(Rect::new(0, 0, 2, 1), 1, 0);
    assert_eq!(pixels, [1, 1, 2, 4, 5, 6]);
}

#[test]
fn stride_separates_rows() {
    let mut pixels = [0_u32; 9];
    // SAFETY: 宽度 2、高度 2、跨度 4，覆盖 8 个像素，缓冲区足够。
    let surface = unsafe { Surface::from_raw(pixels.as_mut_ptr(), 2, 2, 4) };
    surface.fill(Rect::new(0, 0, 2, 2), 1);
    assert_eq!(pixels, [1, 1, 0, 0, 1, 1, 0, 0, 0]);
}

#[test]
#[should_panic(expected = "像素缓冲区小于表面尺寸")]
fn from_slice_rejects_short_buffer() {
    let mut pixels = [0_u32; 3];
    let _ = Surface::from_slice(&mut pixels, 2, 2);
}
