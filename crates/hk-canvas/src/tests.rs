//! 像素原语与文本绘制的单元测试。

use crate::{GlyphSource, Rect, Surface, mix, text_width};

/// 3×3 点阵字体：只覆盖 `A`、`B` 与 `中`。
struct TestFont;

impl GlyphSource for TestFont {
    fn advance(&self, character: char) -> i32 {
        match character {
            'A' | 'B' => 2,
            '中' => 3,
            _ => 1,
        }
    }

    fn for_each_pixel(&self, character: char, mut draw: impl FnMut(i32, i32)) {
        match character {
            'A' => {
                draw(0, 0);
                draw(1, 1);
            }
            'B' => draw(1, 0),
            '中' => {
                draw(1, 0);
                draw(0, 1);
                draw(2, 2);
            }
            _ => {}
        }
    }
}

fn surface(pixels: &mut [u32], width: usize, height: usize) -> Surface<'_> {
    Surface::from_slice(pixels, width, height)
}

#[test]
fn fill_stays_inside_surface() {
    let mut pixels = [0_u32; 12];
    let canvas = surface(&mut pixels, 4, 3);
    canvas.fill(Rect::new(-2, -1, 10, 10), 0x11_2233);
    assert!(pixels.iter().all(|pixel| *pixel == 0x11_2233));
}

#[test]
fn fill_ignores_empty_rect() {
    let mut pixels = [7_u32; 12];
    let canvas = surface(&mut pixels, 4, 3);
    canvas.fill(Rect::new(1, 1, 0, 2), 0);
    canvas.fill(Rect::new(1, 1, 2, -1), 0);
    assert!(pixels.iter().all(|pixel| *pixel == 7));
}

#[test]
fn clip_bounds_every_primitive() {
    let mut pixels = [0_u32; 16];
    let canvas = surface(&mut pixels, 4, 4);
    let window = canvas.clipped(Rect::new(1, 1, 2, 2));
    window.fill(Rect::new(0, 0, 4, 4), 0xFF_FFFF);
    window.put(0, 0, 0);
    window.put(3, 3, 0);
    assert_eq!(window.get(0, 0), None);
    assert_eq!(canvas.get(0, 0), Some(0));
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
    let canvas = surface(&mut pixels, 4, 4);
    let inner = canvas.clipped(Rect::new(1, 1, 2, 2));
    assert_eq!(canvas.clip(), Rect::new(0, 0, 4, 4));
    assert_eq!(inner.clip(), Rect::new(1, 1, 2, 2));
    assert_eq!(
        inner.clipped(Rect::new(2, 2, 8, 8)).clip(),
        Rect::new(2, 2, 1, 1)
    );
}

#[test]
fn mix_interpolates_each_channel() {
    assert_eq!(mix(0x00_00_00, 0xFF_FF_FF, 0), 0x00_00_00);
    assert_eq!(mix(0x00_00_00, 0xFF_FF_FF, 255), 0xFF_FF_FF);
    assert_eq!(mix(0x00_00_00, 0xFF_FF_FF, 128), 0x80_80_80);
    assert_eq!(mix(0x10_20_30, 0x10_20_30, 200), 0x10_20_30);
}

#[test]
fn blend_reads_and_writes_the_same_pixel() {
    let mut pixels = [0x00_00_00_u32; 4];
    let canvas = surface(&mut pixels, 2, 2);
    canvas.blend(1, 1, 0xFF_FF_FF, 51);
    canvas.blend(5, 5, 0xFF_FF_FF, 255);
    assert_eq!(pixels[3], 0x33_33_33);
    assert_eq!(pixels[0], 0);
}

#[test]
fn blend_rect_covers_region() {
    let mut pixels = [0x00_00_00_u32; 9];
    let canvas = surface(&mut pixels, 3, 3);
    canvas.blend_rect(Rect::new(1, 0, 2, 1), 0xFF_FF_FF, 255);
    assert_eq!(pixels[..3], [0, 0xFF_FF_FF, 0xFF_FF_FF]);
    assert_eq!(pixels[3..], [0; 6]);
}

#[test]
fn draw_text_advances_by_glyph_width() {
    let mut pixels = [0_u32; 64];
    let canvas = surface(&mut pixels, 8, 8);
    let mut pen = 0;
    canvas.draw_text(&mut pen, 0, "AB中", 0xFF_FFFF, &TestFont);
    assert_eq!(pen, 7);
    assert_eq!(canvas.get(0, 0), Some(0xFF_FFFF));
    assert_eq!(canvas.get(1, 1), Some(0xFF_FFFF));
    assert_eq!(canvas.get(3, 0), Some(0xFF_FFFF));
    assert_eq!(canvas.get(5, 0), Some(0xFF_FFFF));
    assert_eq!(canvas.get(4, 1), Some(0xFF_FFFF));
    assert_eq!(canvas.get(6, 2), Some(0xFF_FFFF));
    assert_eq!(text_width(&TestFont, "AB中"), 7);
}

#[test]
fn draw_glyph_respects_clip() {
    let mut pixels = [0_u32; 9];
    let canvas = surface(&mut pixels, 3, 3);
    let narrow = canvas.clipped(Rect::new(0, 0, 1, 3));
    narrow.draw_glyph(0, 0, 0xFF, '中', &TestFont);
    assert_eq!(canvas.get(1, 0), Some(0));
    assert_eq!(canvas.get(0, 1), Some(0xFF));
}

#[test]
fn copy_row_clips_to_intersection() {
    let mut pixels = [0_u32; 6];
    let source = [1_u32, 2, 3, 4];
    let canvas = surface(&mut pixels, 3, 2);
    let view = canvas.clipped(Rect::new(1, 0, 2, 2));
    // SAFETY: 源行长度为 4，覆盖整个拷贝区间。
    unsafe { view.copy_row(0, 1, 4, source.as_ptr()) };
    assert_eq!(pixels[3..], [0, 2, 3]);
}

#[test]
fn copy_row_outside_clip_is_noop() {
    let mut pixels = [0_u32; 4];
    let canvas = surface(&mut pixels, 2, 2);
    // SAFETY: 源行长度为 2，未越界。
    unsafe { canvas.copy_row(0, 5, 2, [9_u32, 9].as_ptr()) };
    assert_eq!(pixels, [0; 4]);
}

#[test]
fn stride_separates_rows() {
    let mut pixels = [0_u32; 9];
    // SAFETY: 宽度 2、高度 2、跨度 4，覆盖 8 个像素，缓冲区足够。
    let canvas = unsafe { Surface::from_raw(pixels.as_mut_ptr(), 2, 2, 4) };
    canvas.fill(Rect::new(0, 0, 2, 2), 1);
    assert_eq!(pixels, [1, 1, 0, 0, 1, 1, 0, 0, 0]);
}

#[test]
#[should_panic(expected = "像素缓冲区小于表面尺寸")]
fn from_slice_rejects_short_buffer() {
    let mut pixels = [0_u32; 3];
    let _ = Surface::from_slice(&mut pixels, 2, 2);
}
