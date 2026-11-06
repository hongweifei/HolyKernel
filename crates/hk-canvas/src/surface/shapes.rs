//! 形状图元：圆角、椭圆、线段与渐变。

use crate::color::mix;
use crate::rect::Rect;

use super::{FP_ONE, Surface};

impl Surface<'_> {
    /// 用单色填充圆角矩形。
    ///
    /// 四个角的行边界按圆方程内缩，边界像素按覆盖率混入，边缘不出现台阶。
    /// `radius` 超出短边一半时按一半处理。全程使用整数定点运算。
    pub fn fill_rounded(&self, rect: Rect, radius: i32, color: u32) {
        let radius = clamp_radius(rect, radius);
        if radius == 0 {
            self.fill(rect, color);
            return;
        }
        let area = rect.intersect(self.clip());
        if area.is_empty() {
            return;
        }
        for row in area.y..area.bottom() {
            if let Some((left, right)) = rounded_edges(rect, radius, row) {
                self.fill_span(left, right, row, color);
            }
        }
    }

    /// 用单色描边圆角矩形，`width` 超出短边一半时退化为实心圆角矩形。
    pub fn stroke_rounded(&self, rect: Rect, radius: i32, width: i32, color: u32) {
        if width <= 0 || rect.is_empty() {
            return;
        }
        let limit = rect.width.min(rect.height) / 2;
        let width = width.min(limit.max(0));
        if width == 0 {
            return;
        }
        let radius = clamp_radius(rect, radius);
        let inner = Rect::new(
            rect.x.saturating_add(width),
            rect.y.saturating_add(width),
            rect.width.saturating_sub(width.saturating_mul(2)),
            rect.height.saturating_sub(width.saturating_mul(2)),
        );
        let inner_radius = (radius - width).max(0);
        let area = rect.intersect(self.clip());
        if area.is_empty() {
            return;
        }
        for row in area.y..area.bottom() {
            let Some((outer_left, outer_right)) = rounded_edges(rect, radius, row) else {
                continue;
            };
            match rounded_edges(inner, inner_radius, row) {
                Some((inner_left, inner_right)) => {
                    self.fill_span(outer_left, inner_left, row, color);
                    self.fill_span(inner_right, outer_right, row, color);
                }
                None => self.fill_span(outer_left, outer_right, row, color),
            }
        }
    }

    /// 用单色填充矩形内切椭圆。
    pub fn fill_ellipse(&self, rect: Rect, color: u32) {
        if rect.is_empty() {
            return;
        }
        let area = rect.intersect(self.clip());
        if area.is_empty() {
            return;
        }
        let half_width = rect.width as u128 * 128;
        let half_height = rect.height as u128 * 128;
        if half_width == 0 || half_height == 0 {
            return;
        }
        let center_x = rect.x as i64 * FP_ONE + rect.width as i64 * 128;
        let center_y = rect.y as i64 * FP_ONE + rect.height as i64 * 128;
        for row in area.y..area.bottom() {
            let offset = (row as i64 * FP_ONE + 128 - center_y).unsigned_abs() as u128;
            if offset >= half_height {
                continue;
            }
            let span =
                half_width * isqrt(half_height * half_height - offset * offset) / half_height;
            let left = center_x - span as i64;
            let right = center_x + span as i64;
            self.fill_span(left, right, row, color);
        }
    }

    /// 用单色填充圆。
    pub fn fill_circle(&self, center_x: i32, center_y: i32, radius: i32, color: u32) {
        if radius <= 0 {
            return;
        }
        let diameter = radius.saturating_mul(2);
        self.fill_ellipse(
            Rect::new(
                center_x.saturating_sub(radius),
                center_y.saturating_sub(radius),
                diameter,
                diameter,
            ),
            color,
        );
    }

    /// 在矩形内按行做垂直渐变填充，`top` 在最上一行、`bottom` 在最下一行。
    pub fn fill_vertical_gradient(&self, rect: Rect, top: u32, bottom: u32) {
        let area = rect.intersect(self.clip());
        if area.is_empty() {
            return;
        }
        let height = rect.height.max(1) as u32;
        for row in area.y..area.bottom() {
            let offset = (row - rect.y).max(0) as u32;
            let ratio = (offset * 255 / height).min(255) as u8;
            self.fill_row(area.x, row, area.width, mix(top, bottom, ratio));
        }
    }

    /// 在矩形内按列做水平渐变填充，`left` 在最左列、`right` 在最右列。
    pub fn fill_horizontal_gradient(&self, rect: Rect, left: u32, right: u32) {
        let area = rect.intersect(self.clip());
        if area.is_empty() {
            return;
        }
        let width = rect.width.max(1) as u32;
        for row in area.y..area.bottom() {
            // SAFETY: 行区间在裁剪矩形内，裁剪矩形在表面范围内。
            let pixel = unsafe { self.base.add(row as usize * self.stride + area.x as usize) };
            for column in area.x..area.right() {
                let offset = (column - rect.x).max(0) as u32;
                let ratio = (offset * 255 / width).min(255) as u8;
                // SAFETY: 坐标在裁剪矩形内。
                unsafe {
                    pixel
                        .add((column - area.x) as usize)
                        .write(mix(left, right, ratio))
                };
            }
        }
    }

    /// 沿任意方向做线性渐变填充。
    ///
    /// 渐变轴从 `start` 指向 `end`：起点一侧为 `from`，终点一侧为 `to`，
    /// 轴外按最近端点取色。`start` 与 `end` 重合时整块填充 `from`。
    pub fn fill_linear_gradient(
        &self,
        rect: Rect,
        from: u32,
        to: u32,
        start: (i32, i32),
        end: (i32, i32),
    ) {
        let area = rect.intersect(self.clip());
        if area.is_empty() {
            return;
        }
        let axis_x = end.0 as i64 - start.0 as i64;
        let axis_y = end.1 as i64 - start.1 as i64;
        let length = axis_x * axis_x + axis_y * axis_y;
        if length == 0 {
            self.fill(area, from);
            return;
        }
        let denominator = length.saturating_mul(2);
        let step = (axis_x.saturating_mul(2) << 16) / denominator;
        for row in area.y..area.bottom() {
            let offset_y = row as i64 * 2 + 1 - start.1 as i64 * 2;
            let base = (1 - start.0 as i64 * 2)
                .saturating_mul(axis_x)
                .saturating_add(offset_y.saturating_mul(axis_y));
            let mut position = base.saturating_mul(1 << 16) / denominator;
            // SAFETY: 行区间在裁剪矩形内，裁剪矩形在表面范围内。
            let pixel = unsafe { self.base.add(row as usize * self.stride + area.x as usize) };
            for column in area.x..area.right() {
                let ratio = ((position.clamp(0, 1 << 16) * 255) >> 16) as u8;
                // SAFETY: 坐标在裁剪矩形内。
                unsafe {
                    pixel
                        .add((column - area.x) as usize)
                        .write(mix(from, to, ratio))
                };
                position = position.saturating_add(step);
            }
        }
    }

    /// 用单色描边矩形，`width` 同时限制在宽高之内。
    pub fn stroke(&self, rect: Rect, width: i32, color: u32) {
        if width <= 0 || rect.is_empty() {
            return;
        }
        let width = width.min(rect.width).min(rect.height);
        self.fill(Rect::new(rect.x, rect.y, rect.width, width), color);
        self.fill(
            Rect::new(rect.x, rect.bottom() - width, rect.width, width),
            color,
        );
        self.fill(Rect::new(rect.x, rect.y, width, rect.height), color);
        self.fill(
            Rect::new(rect.right() - width, rect.y, width, rect.height),
            color,
        );
    }

    /// 绘制带抗锯齿的线段。
    pub fn draw_line(&self, from: (i32, i32), to: (i32, i32), color: u32) {
        let (mut x0, mut y0) = from;
        let (mut x1, mut y1) = to;
        let steep = (y1 - y0).unsigned_abs() > (x1 - x0).unsigned_abs();
        if steep {
            core::mem::swap(&mut x0, &mut y0);
            core::mem::swap(&mut x1, &mut y1);
        }
        if x0 > x1 {
            core::mem::swap(&mut x0, &mut x1);
            core::mem::swap(&mut y0, &mut y1);
        }
        let delta_x = x1 - x0;
        if delta_x == 0 {
            self.put(x0, y0, color);
            return;
        }
        let gradient = ((y1 - y0) as i64) << 16;
        let gradient = gradient / delta_x as i64;
        let mut position = (y0 as i64) << 16;
        for x in x0..=x1 {
            let base = (position >> 16) as i32;
            let fraction = ((position >> 8) & 0xFF) as u8;
            if steep {
                self.blend(base, x, color, 255 - fraction);
                self.blend(base + 1, x, color, fraction);
            } else {
                self.blend(x, base, color, 255 - fraction);
                self.blend(x, base + 1, color, fraction);
            }
            position = position.saturating_add(gradient);
        }
    }
}

/// 把圆角半径夹到短边一半。
fn clamp_radius(rect: Rect, radius: i32) -> i32 {
    let limit = rect.width.min(rect.height).max(0) / 2;
    radius.clamp(0, limit)
}

/// 返回圆角矩形在 `row` 行的左右边界（8.8 定点）；该行没有内容时返回 `None`。
fn rounded_edges(rect: Rect, radius: i32, row: i32) -> Option<(i64, i64)> {
    if rect.is_empty() || row < rect.y || row >= rect.bottom() {
        return None;
    }
    let left = rect.x as i64 * FP_ONE;
    let right = rect.right() as i64 * FP_ONE;
    if radius <= 0 {
        return Some((left, right));
    }
    let center = row as i64 * FP_ONE + FP_ONE / 2;
    let top_center = rect.y as i64 * FP_ONE + radius as i64 * FP_ONE;
    let bottom_center = rect.bottom() as i64 * FP_ONE - radius as i64 * FP_ONE;
    let offset = (top_center - center).max(center - bottom_center).max(0) as u128;
    let radius_fp = radius as u128 * FP_ONE as u128;
    if offset >= radius_fp {
        return None;
    }
    let inset = (radius_fp - isqrt(radius_fp * radius_fp - offset * offset)) as i64;
    let left = left + inset;
    let right = right - inset;
    (left < right).then_some((left, right))
}

/// 无浮点、无分配的整数平方根。
fn isqrt(value: u128) -> u128 {
    if value < 2 {
        return value;
    }
    let mut current = value;
    let mut next = current.div_ceil(2);
    while next < current {
        current = next;
        next = (current + value / current) / 2;
    }
    current
}
