//! 像素原语与文本绘制的单元测试。

use hk_font::Font;

use crate::{GlyphMask, Rect, Surface, mix, text_width};

/// `B` 的 2×1 覆盖率：右侧一半覆盖。
const B_COVERAGE: [u8; 2] = [0, 128];

fn surface(pixels: &mut [u32], width: usize, height: usize) -> Surface<'_> {
    Surface::from_slice(pixels, width, height)
}

/// 以覆盖率数据描述一个覆盖整个位图的字形。
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
fn draw_text_advances_by_font_metrics() {
    let font = Font::monospace(16);
    let mut pixels = [0_u32; 32 * 32];
    let canvas = surface(&mut pixels, 32, 32);
    let mut pen = 0;
    canvas.draw_text(&mut pen, 0, "A中", 0xFF_FFFF, &font);
    assert_eq!(pen, 24);
    assert_eq!(text_width(&font, "A中"), 24);
    let painted = (0..16).any(|row| (0..24).any(|column| canvas.get(column, row) != Some(0)));
    assert!(painted, "文本应绘制出像素");
}

#[test]
fn draw_glyph_paints_inside_cell() {
    let font = Font::monospace(16);
    let mut pixels = [0_u32; 32 * 32];
    let canvas = surface(&mut pixels, 32, 32);
    canvas.draw_glyph(0, 0, 0xFF_FFFF, '中', &font);
    let painted = (0..16).any(|row| (0..16).any(|column| canvas.get(column, row) != Some(0)));
    assert!(painted, "字形应落在 16×16 单元格内");
    let outside = (0..32).any(|row| (16..32).any(|column| canvas.get(column, row) != Some(0)));
    assert!(!outside, "半宽之外的列不应有像素");
}

#[test]
fn draw_mask_blends_partial_coverage() {
    let mut pixels = [0_u32; 4];
    let canvas = surface(&mut pixels, 2, 1);
    canvas.draw_mask(0, 0, 0xFF_FFFF, &mask(&B_COVERAGE, 2, 1, 0, 0));
    assert_eq!(canvas.get(0, 0), Some(0));
    assert_eq!(canvas.get(1, 0), Some(0x80_8080));
}

#[test]
fn draw_mask_respects_clip() {
    const COVERAGE: [u8; 2] = [255, 255];
    let mut pixels = [0_u32; 9];
    let canvas = surface(&mut pixels, 3, 3);
    let narrow = canvas.clipped(Rect::new(0, 0, 1, 3));
    narrow.draw_mask(0, 1, 0xFF, &mask(&COVERAGE, 2, 1, 0, 0));
    assert_eq!(canvas.get(0, 1), Some(0xFF));
    assert_eq!(canvas.get(1, 1), Some(0), "裁剪外不应绘制");
}

#[test]
fn draw_mask_applies_origin() {
    const COVERAGE: [u8; 1] = [255];
    let mut pixels = [0_u32; 9];
    let canvas = surface(&mut pixels, 3, 3);
    canvas.draw_mask(0, 0, 0xFF, &mask(&COVERAGE, 1, 1, 1, 2));
    assert_eq!(canvas.get(1, 2), Some(0xFF));
    assert_eq!(canvas.get(0, 0), Some(0));
}

#[test]
fn draw_mask_fills_full_coverage_runs() {
    const COVERAGE: [u8; 8] = [255, 255, 255, 255, 255, 255, 255, 255];
    let mut pixels = [0_u32; 8];
    let canvas = surface(&mut pixels, 8, 1);
    canvas.draw_mask(0, 0, 0xFF_FFFF, &mask(&COVERAGE, 8, 1, 0, 0));
    assert_eq!(pixels, [0xFF_FFFF; 8]);
}

/// 对比覆盖率位图绘制与逐像素绘制的吞吐，手动运行：
/// `cargo test -p hk-canvas --release -- --ignored --nocapture bench_text_throughput`
#[test]
#[ignore = "性能测量"]
fn bench_text_throughput() {
    use std::time::Instant;

    const WIDTH: usize = 800;
    const HEIGHT: usize = 600;
    const ROUNDS: usize = 20;
    const LINES: usize = 20;

    let font = Font::new(16);
    let samples = [
        ("中文", "渲染性能测量用的是整行汉字与标点，覆盖位图字形路径"),
        ("ASCII", "Rendering throughput measurement for bitmap path."),
        (
            "混排",
            "HolyKernel 渲染性能测量 performance measurement 混排",
        ),
    ];
    let mut pixels = std::vec![0_u32; WIDTH * HEIGHT];

    for (name, text) in samples {
        let glyphs = (ROUNDS * LINES * text.chars().count()) as u128;
        let mut masked = u128::MAX;
        let mut pixelwise = u128::MAX;
        for _ in 0..ROUNDS {
            let surface = Surface::from_slice(&mut pixels, WIDTH, HEIGHT);
            let start = Instant::now();
            for row in 0..LINES {
                let mut pen = 4;
                surface.draw_text(&mut pen, 8 + row as i32 * 20, text, 0xFF_FFFF, &font);
            }
            masked = masked.min(start.elapsed().as_nanos());

            let surface = Surface::from_slice(&mut pixels, WIDTH, HEIGHT);
            let start = Instant::now();
            for row in 0..LINES {
                let mut pen = 4;
                for character in text.chars() {
                    if let Some(glyph) = font.glyph(character) {
                        let y = 8 + row as i32 * 20;
                        glyph.for_each_pixel(|x, offset_y| {
                            surface.put(pen + x, y + offset_y, 0xFF_FFFF);
                        });
                    }
                    pen += font.advance(character);
                }
            }
            pixelwise = pixelwise.min(start.elapsed().as_nanos());
        }
        std::println!(
            "{name}：按行位图 {} ns/字符，逐像素 {} ns/字符",
            masked / glyphs,
            pixelwise / glyphs
        );
    }
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
