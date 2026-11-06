//! 像素原语与文本绘制的单元测试。

use crate::{GlyphMask, GlyphSource, Rect, Surface, mix, text_width};

/// `A` 的 2×2 覆盖率：左上与右下。
const A_COVERAGE: [u8; 4] = [255, 0, 0, 255];
/// `B` 的 2×1 覆盖率：右侧一半覆盖。
const B_COVERAGE: [u8; 2] = [0, 128];
/// `中` 的 3×3 覆盖率：上中、左中、右下。
const ZHONG_COVERAGE: [u8; 9] = [0, 255, 0, 255, 0, 0, 0, 0, 255];

/// 点阵测试字体：只覆盖 `A`、`B` 与 `中`。
struct TestFont;

impl GlyphSource for TestFont {
    fn advance(&self, character: char) -> i32 {
        match character {
            'A' | 'B' => 2,
            '中' => 3,
            _ => 1,
        }
    }

    fn with_glyph(&self, character: char, mut plot: impl FnMut(&GlyphMask<'_>)) {
        let (coverage, width, height) = match character {
            'A' => (&A_COVERAGE[..], 2, 2),
            'B' => (&B_COVERAGE[..], 2, 1),
            '中' => (&ZHONG_COVERAGE[..], 3, 3),
            _ => return,
        };
        plot(&GlyphMask {
            width,
            height,
            left: 0,
            top: 0,
            stride: width,
            coverage,
        });
    }
}

fn surface(pixels: &mut [u32], width: usize, height: usize) -> Surface<'_> {
    Surface::from_slice(pixels, width, height)
}

/// 带原点的 1×1 字形，用于验证位图偏移。
struct ShiftedFont;

impl GlyphSource for ShiftedFont {
    fn advance(&self, _: char) -> i32 {
        1
    }

    fn with_glyph(&self, _: char, mut plot: impl FnMut(&GlyphMask<'_>)) {
        const COVERAGE: [u8; 1] = [255];
        plot(&GlyphMask {
            width: 1,
            height: 1,
            left: 1,
            top: 2,
            stride: 1,
            coverage: &COVERAGE,
        });
    }
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
fn stroke_covers_border_only() {
    let mut pixels = [0_u32; 16];
    let canvas = surface(&mut pixels, 4, 4);
    canvas.stroke(Rect::new(0, 0, 4, 4), 1, 0xFF);
    for (index, pixel) in pixels.iter().enumerate() {
        let row = index / 4;
        let column = index % 4;
        let border = row == 0 || row == 3 || column == 0 || column == 3;
        let expected = if border { 0xFF } else { 0 };
        assert_eq!(*pixel, expected, "像素 ({column}, {row})");
    }
}

#[test]
fn stroke_width_is_clamped_to_rect() {
    let mut pixels = [0_u32; 4];
    let canvas = surface(&mut pixels, 2, 2);
    canvas.stroke(Rect::new(0, 0, 2, 2), 9, 0xFF);
    assert_eq!(pixels, [0xFF; 4]);
}

#[test]
fn vertical_gradient_runs_from_top_to_bottom() {
    let mut pixels = [0_u32; 4];
    let canvas = surface(&mut pixels, 2, 2);
    canvas.fill_vertical_gradient(Rect::new(0, 0, 2, 2), 0x00_00_00, 0xFF_FF_FF);
    assert_eq!(pixels[..2], [0x00_00_00, 0x00_00_00]);
    assert_eq!(pixels[2..], [0x7F_7F_7F, 0x7F_7F_7F]);
}

#[test]
fn rounded_fill_antialiases_corners() {
    let mut pixels = [0_u32; 16];
    let canvas = surface(&mut pixels, 4, 4);
    canvas.fill_rounded(Rect::new(0, 0, 4, 4), 2, 0xFF_FFFF);
    assert_ne!(pixels[0], 0, "角像素应有部分覆盖");
    assert_ne!(pixels[0], 0xFF_FFFF, "角像素不应完全覆盖");
    assert_eq!(pixels[1], 0xFF_FFFF, "顶行中间像素完全覆盖");
    assert_eq!(pixels[3], pixels[0], "右上角与左上角对称");
    assert_eq!(pixels[12], pixels[0], "左下角与左上角对称");
}

#[test]
fn rounded_fill_with_zero_radius_is_a_rect() {
    let mut pixels = [0_u32; 4];
    let canvas = surface(&mut pixels, 2, 2);
    canvas.fill_rounded(Rect::new(0, 0, 2, 2), 0, 0xFF);
    assert_eq!(pixels, [0xFF; 4]);
}

#[test]
fn blit_copies_region() {
    let mut source_pixels = [1_u32, 2, 3, 4];
    let source = surface(&mut source_pixels, 2, 2);
    let mut target_pixels = [0_u32; 16];
    let canvas = surface(&mut target_pixels, 4, 4);
    canvas.blit(1, 1, &source);
    assert_eq!(target_pixels[5], 1);
    assert_eq!(target_pixels[6], 2);
    assert_eq!(target_pixels[9], 3);
    assert_eq!(target_pixels[10], 4);
    assert_eq!(target_pixels[4], 0);
}

#[test]
fn blit_respects_both_clips() {
    let mut source_pixels = [1_u32, 2, 3, 4];
    let source = surface(&mut source_pixels, 2, 2);
    let mut target_pixels = [0_u32; 16];
    let canvas = surface(&mut target_pixels, 4, 4);
    let window = canvas.clipped(Rect::new(2, 2, 2, 2));
    window.blit(1, 1, &source);
    assert_eq!(target_pixels[10], 4, "只有落在裁剪内的像素被拷贝");
    assert_eq!(target_pixels[5], 0);
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
    assert_eq!(canvas.get(2, 0), Some(0));
    assert_eq!(canvas.get(3, 0), Some(0x80_8080));
    assert_eq!(canvas.get(5, 0), Some(0xFF_FFFF));
    assert_eq!(canvas.get(4, 1), Some(0xFF_FFFF));
    assert_eq!(canvas.get(6, 2), Some(0xFF_FFFF));
    assert_eq!(text_width(&TestFont, "AB中"), 7);
}

#[test]
fn draw_mask_blends_partial_coverage() {
    let mut pixels = [0_u32; 4];
    let canvas = surface(&mut pixels, 2, 1);
    canvas.draw_glyph(0, 0, 0xFF_FFFF, 'B', &TestFont);
    assert_eq!(canvas.get(0, 0), Some(0));
    assert_eq!(canvas.get(1, 0), Some(0x80_8080));
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
fn draw_glyph_offsets_by_mask_origin() {
    let mut pixels = [0_u32; 9];
    let canvas = surface(&mut pixels, 3, 3);
    canvas.draw_glyph(0, 0, 0xFF, 'x', &ShiftedFont);
    assert_eq!(canvas.get(1, 2), Some(0xFF));
    assert_eq!(canvas.get(0, 0), Some(0));
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
