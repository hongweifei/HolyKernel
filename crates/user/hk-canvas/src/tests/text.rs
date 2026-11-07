//! 文本落位、裁剪与省略号测试。

use hk_font::Font;

use crate::{
    Surface, default_font, default_font_scaled, default_monospace, text_width, text_width_default,
};

use super::{canvas, mask};

/// `B` 的 2×1 覆盖率：右侧一半覆盖。
const B_COVERAGE: [u8; 2] = [0, 128];

#[test]
fn draw_text_advances_by_font_metrics() {
    let font = Font::monospace(16);
    let mut pixels = [0_u32; 32 * 32];
    let surface = canvas(&mut pixels, 32, 32);
    let mut pen = 0;
    surface.draw_text(&mut pen, 0, "A中", 0xFF_FFFF, &font);
    assert_eq!(pen, 24);
    assert_eq!(text_width(&font, "A中"), 24);
    let painted = (0..16).any(|row| (0..24).any(|column| surface.get(column, row) != Some(0)));
    assert!(painted, "文本应绘制出像素");
}

#[test]
fn draw_text_default_matches_explicit_default_font() {
    let font = default_font();
    let mut explicit_pixels = [0_u32; 64 * 16];
    let explicit = canvas(&mut explicit_pixels, 64, 16);
    let mut explicit_pen = 0;
    explicit.draw_text(&mut explicit_pen, 0, "A中", 0xFF_FFFF, font);

    let mut default_pixels = [0_u32; 64 * 16];
    let default = canvas(&mut default_pixels, 64, 16);
    let mut default_pen = 0;
    default.draw_text_default(&mut default_pen, 0, "A中", 0xFF_FFFF);

    assert_eq!(default_pen, explicit_pen);
    assert_eq!(default_pixels, explicit_pixels);
    assert_eq!(text_width_default("A中"), text_width(font, "A中"));
    assert!(default_pen > 0);
}

#[test]
fn draw_glyph_default_paints_without_a_font_argument() {
    let mut pixels = [0_u32; 64 * 16];
    let surface = canvas(&mut pixels, 64, 16);
    surface.draw_glyph_default(0, 0, 0xFF_FFFF, '中');
    let painted = (0..16).any(|row| (0..16).any(|column| surface.get(column, row) != Some(0)));
    assert!(painted, "内置字体应绘制出像素");
}

#[test]
fn default_font_is_initialized_once() {
    assert!(core::ptr::eq(default_font(), default_font()));
}

#[test]
fn default_monospace_keeps_fixed_grid_metrics() {
    let font = default_monospace();
    assert!(core::ptr::eq(font, default_monospace()));
    assert_eq!(font.advance('A'), 8);
    assert_eq!(font.advance('中'), 16);
}

#[test]
fn default_font_scaled_covers_ui_scale() {
    assert!(core::ptr::eq(default_font_scaled(1), default_font()));
    let large = default_font_scaled(2);
    assert!(core::ptr::eq(large, default_font_scaled(2)));
    assert_eq!(large.size(), 32);
    assert!(large.has_scalable());
}

#[test]
fn draw_text_clipped_stops_at_right_edge() {
    let font = Font::monospace(16);
    let mut pixels = [0_u32; 64 * 16];
    let surface = canvas(&mut pixels, 64, 16);
    let full = text_width(&font, "AAAA");
    let mut pen = 0;
    surface.draw_text_clipped(&mut pen, 0, "AAAA", full / 2, 0xFF_FFFF, &font);
    assert!(pen > 0);
    assert!(pen < full, "裁剪后应提前停止");
    assert!(pen <= full / 2 + font.advance('A'), "最多越过边界一个字");
}

#[test]
fn draw_text_ellipsis_truncates_long_text() {
    let font = Font::monospace(16);
    let mut pixels = [0_u32; 64 * 16];
    let surface = canvas(&mut pixels, 64, 16);
    let full = text_width(&font, "AAAAAAAA");
    let mut pen = 0;
    surface.draw_text_ellipsis(&mut pen, 0, "AAAAAAAA", full / 2, 0xFF_FFFF, &font);
    assert!(pen > 0);
    assert!(pen <= full / 2, "省略号应落在给定宽度内");
}

#[test]
fn draw_text_ellipsis_keeps_short_text_intact() {
    let font = Font::monospace(16);
    let mut pixels = [0_u32; 64 * 16];
    let surface = canvas(&mut pixels, 64, 16);
    let mut pen = 0;
    surface.draw_text_ellipsis(&mut pen, 0, "A中", 200, 0xFF_FFFF, &font);
    assert_eq!(pen, text_width(&font, "A中"));
}

#[test]
fn draw_glyph_paints_inside_cell() {
    let font = Font::monospace(16);
    let mut pixels = [0_u32; 32 * 32];
    let surface = canvas(&mut pixels, 32, 32);
    surface.draw_glyph(0, 0, 0xFF_FFFF, '中', &font);
    let painted = (0..16).any(|row| (0..16).any(|column| surface.get(column, row) != Some(0)));
    assert!(painted, "字形应落在 16×16 单元格内");
    let outside = (0..32).any(|row| (16..32).any(|column| surface.get(column, row) != Some(0)));
    assert!(!outside, "半宽之外的列不应有像素");
}

#[test]
fn draw_mask_blends_partial_coverage() {
    let mut pixels = [0_u32; 4];
    let surface = canvas(&mut pixels, 2, 1);
    surface.draw_mask(0, 0, 0xFF_FFFF, &mask(&B_COVERAGE, 2, 1, 0, 0));
    assert_eq!(surface.get(0, 0), Some(0));
    assert_eq!(surface.get(1, 0), Some(0x80_8080));
}

#[test]
fn draw_mask_respects_clip() {
    const COVERAGE: [u8; 2] = [255, 255];
    let mut pixels = [0_u32; 9];
    let surface = canvas(&mut pixels, 3, 3);
    let narrow = surface.clipped(crate::Rect::new(0, 0, 1, 3));
    narrow.draw_mask(0, 1, 0xFF, &mask(&COVERAGE, 2, 1, 0, 0));
    assert_eq!(surface.get(0, 1), Some(0xFF));
    assert_eq!(surface.get(1, 1), Some(0), "裁剪外不应绘制");
}

#[test]
fn draw_mask_applies_origin() {
    const COVERAGE: [u8; 1] = [255];
    let mut pixels = [0_u32; 9];
    let surface = canvas(&mut pixels, 3, 3);
    surface.draw_mask(0, 0, 0xFF, &mask(&COVERAGE, 1, 1, 1, 2));
    assert_eq!(surface.get(1, 2), Some(0xFF));
    assert_eq!(surface.get(0, 0), Some(0));
}

#[test]
fn draw_mask_fills_full_coverage_runs() {
    const COVERAGE: [u8; 8] = [255, 255, 255, 255, 255, 255, 255, 255];
    let mut pixels = [0_u32; 8];
    let surface = canvas(&mut pixels, 8, 1);
    surface.draw_mask(0, 0, 0xFF_FFFF, &mask(&COVERAGE, 8, 1, 0, 0));
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

/// 测量填充、混合与缩放的吞吐，手动运行：
/// `cargo test -p hk-canvas --release -- --ignored --nocapture bench_primitive_throughput`
#[test]
#[ignore = "性能测量"]
fn bench_primitive_throughput() {
    use std::time::Instant;

    const WIDTH: usize = 800;
    const HEIGHT: usize = 600;
    const ROUNDS: usize = 20;

    let mut pixels = std::vec![0_u32; WIDTH * HEIGHT];
    let mut source_pixels = std::vec![0x80_8080_u32; 400 * 300];
    let source = Surface::from_slice(&mut source_pixels, 400, 300);
    let (mut fill, mut blend, mut scale) = (u128::MAX, u128::MAX, u128::MAX);

    for _ in 0..ROUNDS {
        let surface = Surface::from_slice(&mut pixels, WIDTH, HEIGHT);
        let start = Instant::now();
        surface.fill(
            crate::Rect::new(0, 0, WIDTH as i32, HEIGHT as i32),
            0x12_3456,
        );
        fill = fill.min(start.elapsed().as_nanos());

        let surface = Surface::from_slice(&mut pixels, WIDTH, HEIGHT);
        let start = Instant::now();
        surface.blend_rect(
            crate::Rect::new(0, 0, WIDTH as i32, HEIGHT as i32),
            0x34_5678,
            128,
        );
        blend = blend.min(start.elapsed().as_nanos());

        let surface = Surface::from_slice(&mut pixels, WIDTH, HEIGHT);
        let start = Instant::now();
        surface.blit_scaled(0, 0, WIDTH as i32, HEIGHT as i32, &source);
        scale = scale.min(start.elapsed().as_nanos());
    }

    let bytes = (WIDTH * HEIGHT * 4) as f64;
    let mib = |nanos: u128| bytes * 1e9 / (nanos as f64) / (1024.0 * 1024.0);
    std::println!(
        "填充 {:.1} MiB/s，混合 {:.1} MiB/s，缩放 {:.1} MiB/s",
        mib(fill),
        mib(blend),
        mib(scale)
    );
}
