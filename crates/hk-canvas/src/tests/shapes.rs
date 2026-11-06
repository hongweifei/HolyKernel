//! 圆角、椭圆、线段与渐变测试。

use crate::Rect;

use super::canvas;

#[test]
fn stroke_covers_border_only() {
    let mut pixels = [0_u32; 16];
    let surface = canvas(&mut pixels, 4, 4);
    surface.stroke(Rect::new(0, 0, 4, 4), 1, 0xFF);
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
    let surface = canvas(&mut pixels, 2, 2);
    surface.stroke(Rect::new(0, 0, 2, 2), 9, 0xFF);
    assert_eq!(pixels, [0xFF; 4]);
}

#[test]
fn vertical_gradient_runs_from_top_to_bottom() {
    let mut pixels = [0_u32; 4];
    let surface = canvas(&mut pixels, 2, 2);
    surface.fill_vertical_gradient(Rect::new(0, 0, 2, 2), 0x00_00_00, 0xFF_FF_FF);
    assert_eq!(pixels[..2], [0x00_00_00, 0x00_00_00]);
    assert_eq!(pixels[2..], [0x7F_7F_7F, 0x7F_7F_7F]);
}

#[test]
fn horizontal_gradient_runs_from_left_to_right() {
    let mut pixels = [0_u32; 2];
    let surface = canvas(&mut pixels, 2, 1);
    surface.fill_horizontal_gradient(Rect::new(0, 0, 2, 1), 0x00_00_00, 0xFF_FF_FF);
    assert_eq!(pixels, [0x00_00_00, 0x7F_7F_7F]);
}

#[test]
fn linear_gradient_follows_its_axis() {
    let mut pixels = [0_u32; 2];
    let surface = canvas(&mut pixels, 2, 1);
    surface.fill_linear_gradient(
        Rect::new(0, 0, 2, 1),
        0x00_00_00,
        0xFF_FF_FF,
        (0, 0),
        (2, 0),
    );
    assert_eq!(pixels, [0x3F_3F3F, 0xBF_BFBF]);
}

#[test]
fn linear_gradient_with_zero_axis_is_solid() {
    let mut pixels = [0_u32; 2];
    let surface = canvas(&mut pixels, 2, 1);
    surface.fill_linear_gradient(Rect::new(0, 0, 2, 1), 0x12_3456, 0xFF_FFFF, (1, 1), (1, 1));
    assert_eq!(pixels, [0x12_3456, 0x12_3456]);
}

#[test]
fn rounded_fill_antialiases_corners() {
    let mut pixels = [0_u32; 16];
    let surface = canvas(&mut pixels, 4, 4);
    surface.fill_rounded(Rect::new(0, 0, 4, 4), 2, 0xFF_FFFF);
    assert_ne!(pixels[0], 0, "角像素应有部分覆盖");
    assert_ne!(pixels[0], 0xFF_FFFF, "角像素不应完全覆盖");
    assert_eq!(pixels[1], 0xFF_FFFF, "顶行中间像素完全覆盖");
    assert_eq!(pixels[3], pixels[0], "右上角与左上角对称");
    assert_eq!(pixels[12], pixels[0], "左下角与左上角对称");
}

#[test]
fn rounded_fill_with_zero_radius_is_a_rect() {
    let mut pixels = [0_u32; 4];
    let surface = canvas(&mut pixels, 2, 2);
    surface.fill_rounded(Rect::new(0, 0, 2, 2), 0, 0xFF);
    assert_eq!(pixels, [0xFF; 4]);
}

#[test]
fn rounded_stroke_keeps_the_center_empty() {
    let mut pixels = [0_u32; 64];
    let surface = canvas(&mut pixels, 8, 8);
    surface.stroke_rounded(Rect::new(0, 0, 8, 8), 2, 1, 0xFF_FFFF);
    assert_eq!(surface.get(4, 4), Some(0), "中心应保持透明");
    assert_eq!(surface.get(4, 0), Some(0xFF_FFFF), "上边应为实线");
    assert_eq!(surface.get(0, 4), Some(0xFF_FFFF), "左边中部为实线边框");
    let corner = surface.get(0, 0).unwrap();
    assert_ne!(corner, 0, "圆角处应有部分覆盖");
    assert_ne!(corner, 0xFF_FFFF, "圆角外缘不应完全覆盖");
}

#[test]
fn circle_fills_center_and_leaves_corners_empty() {
    let mut pixels = [0_u32; 256];
    let surface = canvas(&mut pixels, 16, 16);
    surface.fill_circle(8, 8, 6, 0xFF_FFFF);
    assert_eq!(surface.get(8, 8), Some(0xFF_FFFF), "圆心应填满");
    assert_eq!(surface.get(8, 2), Some(0xFF_FFFF), "上半部仍在圆内");
    assert_eq!(surface.get(0, 0), Some(0), "外接矩形角落应在圆外");
    assert_eq!(surface.get(8, 1), Some(0), "圆顶点之外的一行应为空");
}

#[test]
fn ellipse_fills_center_and_leaves_corners_empty() {
    let mut pixels = [0_u32; 8 * 4];
    let surface = canvas(&mut pixels, 8, 4);
    surface.fill_ellipse(Rect::new(0, 0, 8, 4), 0xFF_FFFF);
    assert_eq!(surface.get(4, 2), Some(0xFF_FFFF), "椭圆中心应填满");
    assert_eq!(surface.get(0, 0), Some(0), "外接矩形角落应在椭圆外");
}

#[test]
fn draw_line_paints_axis_aligned_runs() {
    let mut pixels = [0_u32; 8];
    let surface = canvas(&mut pixels, 8, 1);
    surface.draw_line((0, 0), (7, 0), 0xFF_FFFF);
    assert_eq!(pixels, [0xFF_FFFF; 8]);
}

#[test]
fn draw_line_paints_vertical_column() {
    let mut pixels = [0_u32; 8 * 4];
    let surface = canvas(&mut pixels, 8, 4);
    surface.draw_line((2, 0), (2, 3), 0xFF_FFFF);
    for row in 0..4 {
        assert_eq!(surface.get(2, row), Some(0xFF_FFFF));
    }
    assert_eq!(surface.get(1, 0), Some(0));
}

#[test]
fn draw_line_paints_diagonal_cells() {
    let mut pixels = [0_u32; 16];
    let surface = canvas(&mut pixels, 4, 4);
    surface.draw_line((0, 0), (3, 3), 0xFF_FFFF);
    for index in 0..4 {
        assert_eq!(surface.get(index, index), Some(0xFF_FFFF));
        assert_eq!(surface.get(index, (index + 1) % 4), Some(0));
    }
}
