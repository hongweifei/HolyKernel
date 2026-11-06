//! 表面、裁剪与像素原语。

use core::cell::Cell;
use core::marker::PhantomData;

/// 表面上的整数矩形。
///
/// 宽高为非正值时视为空矩形，所有绘制原语都会跳过它。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    /// 左边界（含）。
    pub x: i32,
    /// 上边界（含）。
    pub y: i32,
    /// 宽度。
    pub width: i32,
    /// 高度。
    pub height: i32,
}

impl Rect {
    /// 以左上角与宽高构造矩形。
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// 矩形是否为空。
    pub const fn is_empty(self) -> bool {
        self.width <= 0 || self.height <= 0
    }

    /// 右边界（不含）。
    pub const fn right(self) -> i32 {
        self.x.saturating_add(self.width)
    }

    /// 下边界（不含）。
    pub const fn bottom(self) -> i32 {
        self.y.saturating_add(self.height)
    }

    /// 点是否落在矩形内。
    pub const fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }

    /// 与另一个矩形求交；无交集时返回空矩形。
    pub const fn intersect(self, other: Rect) -> Rect {
        let x = if self.x > other.x { self.x } else { other.x };
        let y = if self.y > other.y { self.y } else { other.y };
        let right = if self.right() < other.right() {
            self.right()
        } else {
            other.right()
        };
        let bottom = if self.bottom() < other.bottom() {
            self.bottom()
        } else {
            other.bottom()
        };
        let width = right.saturating_sub(x);
        let height = bottom.saturating_sub(y);
        Rect {
            x,
            y,
            width: if width > 0 { width } else { 0 },
            height: if height > 0 { height } else { 0 },
        }
    }
}

/// 一块可绘制的像素缓冲区。
///
/// 表面只持视图形状：`base` 指向第一行首像素，行之间相隔 `stride` 个像素。
/// 所有绘制方法取 `&self`，因为同一块缓冲区上可能同时存在多个裁剪视图；
/// 写入经内部指针完成，语义与 [`Cell`] 一致。
#[derive(Clone, Copy)]
pub struct Surface<'a> {
    base: *mut u32,
    width: usize,
    height: usize,
    stride: usize,
    clip: Rect,
    marker: PhantomData<&'a Cell<u32>>,
}

impl<'a> Surface<'a> {
    /// 从裸指针构造整块表面。
    ///
    /// # Safety
    ///
    /// `base` 必须指向至少 `stride * height` 个可写像素，并在表面存续期间保持有效。
    pub const unsafe fn from_raw(
        base: *mut u32,
        width: usize,
        height: usize,
        stride: usize,
    ) -> Surface<'a> {
        assert!(width <= i32::MAX as usize, "表面宽度超出 i32 坐标范围");
        assert!(height <= i32::MAX as usize, "表面高度超出 i32 坐标范围");
        assert!(stride >= width, "行跨度小于宽度");
        Surface {
            base,
            width,
            height,
            stride,
            clip: Rect::new(0, 0, width as i32, height as i32),
            marker: PhantomData,
        }
    }

    /// 从像素切片构造整块表面，行跨度等于宽度。
    ///
    /// # Panics
    ///
    /// 切片长度不足 `width * height` 时停机。
    pub fn from_slice(pixels: &'a mut [u32], width: usize, height: usize) -> Surface<'a> {
        let needed = width.checked_mul(height).expect("表面尺寸溢出");
        assert!(pixels.len() >= needed, "像素缓冲区小于表面尺寸");
        // SAFETY: 长度已校验，切片在表面存续期间有效。
        unsafe { Surface::from_raw(pixels.as_mut_ptr(), width, height, width) }
    }

    /// 表面宽度。
    pub const fn width(&self) -> usize {
        self.width
    }

    /// 表面高度。
    pub const fn height(&self) -> usize {
        self.height
    }

    /// 行跨度。
    pub const fn stride(&self) -> usize {
        self.stride
    }

    /// 当前裁剪矩形。
    pub const fn clip(&self) -> Rect {
        self.clip
    }

    /// 返回首行首像素指针，行之间相隔 [`Surface::stride`] 个像素。
    ///
    /// 指针不受裁剪约束，越界判定由调用方负责。
    pub const fn base(&self) -> *mut u32 {
        self.base
    }

    /// 派生一个裁剪到 `rect` 的视图，与已有裁剪求交。
    pub fn clipped(&self, rect: Rect) -> Surface<'a> {
        let mut view = *self;
        view.clip = self.clip.intersect(rect);
        view
    }

    /// 写一个像素；落在裁剪外时跳过。
    pub fn put(&self, x: i32, y: i32, color: u32) {
        if !self.clip.contains(x, y) {
            return;
        }
        // SAFETY: 裁剪矩形在表面范围内，坐标因此有效。
        unsafe {
            self.base
                .add(y as usize * self.stride + x as usize)
                .write(color);
        }
    }

    /// 读一个像素；落在裁剪外时返回 `None`。
    pub fn get(&self, x: i32, y: i32) -> Option<u32> {
        if !self.clip.contains(x, y) {
            return None;
        }
        // SAFETY: 裁剪矩形在表面范围内，坐标因此有效。
        Some(unsafe { self.base.add(y as usize * self.stride + x as usize).read() })
    }

    /// 用单色填充矩形。
    pub fn fill(&self, rect: Rect, color: u32) {
        let area = rect.intersect(self.clip);
        if area.is_empty() {
            return;
        }
        for row in area.y..area.bottom() {
            // SAFETY: 行区间在裁剪矩形内，裁剪矩形在表面范围内。
            unsafe {
                core::slice::from_raw_parts_mut(
                    self.base.add(row as usize * self.stride + area.x as usize),
                    area.width as usize,
                )
                .fill(color);
            }
        }
    }

    /// 按 `ratio/255` 把 `color` 混入一个像素；落在裁剪外时跳过。
    pub fn blend(&self, x: i32, y: i32, color: u32, ratio: u8) {
        if !self.clip.contains(x, y) {
            return;
        }
        // SAFETY: 裁剪矩形在表面范围内，坐标因此有效。
        unsafe {
            let pixel = self.base.add(y as usize * self.stride + x as usize);
            pixel.write(mix(pixel.read(), color, ratio));
        }
    }

    /// 按 `ratio/255` 把 `color` 混入矩形。
    pub fn blend_rect(&self, rect: Rect, color: u32, ratio: u8) {
        let area = rect.intersect(self.clip);
        for row in area.y..area.bottom() {
            for column in area.x..area.right() {
                // SAFETY: 坐标在裁剪矩形内，裁剪矩形在表面范围内。
                unsafe {
                    let pixel = self.base.add(row as usize * self.stride + column as usize);
                    pixel.write(mix(pixel.read(), color, ratio));
                }
            }
        }
    }

    /// 用单色填充圆角矩形。
    ///
    /// 四个角的行边界按圆方程内缩，边界像素按覆盖率混入，边缘不出现台阶。
    /// `radius` 超出短边一半时按一半处理。
    pub fn fill_rounded(&self, rect: Rect, radius: i32, color: u32) {
        let limit = rect.width.min(rect.height) / 2;
        let radius = radius.max(0).min(limit);
        if radius == 0 {
            self.fill(rect, color);
            return;
        }
        let area = rect.intersect(self.clip);
        if area.is_empty() {
            return;
        }
        let radius_f = radius as f32;
        let top_center = rect.y as f32 + radius_f;
        let bottom_center = rect.bottom() as f32 - radius_f;
        for row in area.y..area.bottom() {
            let center = row as f32 + 0.5;
            let dy = (top_center - center).max(center - bottom_center);
            let inset = if dy <= 0.0 {
                0.0
            } else if dy >= radius_f {
                radius_f
            } else {
                radius_f - libm::sqrtf(radius_f * radius_f - dy * dy)
            };
            let left_edge = rect.x as f32 + inset;
            let right_edge = rect.right() as f32 - inset;
            if left_edge >= right_edge {
                continue;
            }
            let full_left = libm::ceilf(left_edge);
            let full_right = libm::floorf(right_edge);
            let left_pixel = full_left as i32;
            let right_pixel = full_right as i32;
            if left_pixel < right_pixel {
                self.fill(
                    Rect::new(left_pixel, row, right_pixel - left_pixel, 1),
                    color,
                );
            }
            let left_part = full_left - left_edge;
            if left_part > 0.0 {
                self.blend(
                    left_pixel - 1,
                    row,
                    color,
                    libm::roundf(left_part * 255.0) as u8,
                );
            }
            let right_part = right_edge - full_right;
            if right_part > 0.0 {
                self.blend(
                    right_pixel,
                    row,
                    color,
                    libm::roundf(right_part * 255.0) as u8,
                );
            }
        }
    }

    /// 在矩形内按行做垂直渐变填充，`top` 在最上一行、`bottom` 在最下一行。
    pub fn fill_vertical_gradient(&self, rect: Rect, top: u32, bottom: u32) {
        let area = rect.intersect(self.clip);
        if area.is_empty() {
            return;
        }
        let height = rect.height.max(1) as u32;
        for row in area.y..area.bottom() {
            let offset = (row - rect.y).max(0) as u32;
            let ratio = (offset * 255 / height).min(255) as u8;
            self.fill(
                Rect::new(area.x, row, area.width, 1),
                mix(top, bottom, ratio),
            );
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

    /// 把另一个表面整块拷贝到 `(at_x, at_y)`，按两层裁剪求交。
    pub fn blit(&self, at_x: i32, at_y: i32, source: &Surface<'_>) {
        let target = Rect::new(at_x, at_y, source.width as i32, source.height as i32);
        let area = target.intersect(self.clip);
        if area.is_empty() {
            return;
        }
        let source_rect = Rect::new(
            area.x.saturating_sub(at_x),
            area.y.saturating_sub(at_y),
            area.width,
            area.height,
        );
        let source_area = source_rect.intersect(source.clip);
        if source_area.is_empty() {
            return;
        }
        let target_left = source_area.x.saturating_add(at_x);
        let target_top = source_area.y.saturating_add(at_y);
        for row in 0..source_area.height as usize {
            // SAFETY: 源行与目标行都落在各自表面的裁剪矩形内，长度按交集宽度裁剪。
            unsafe {
                core::ptr::copy_nonoverlapping(
                    source.base.add(
                        (source_area.y as usize + row) * source.stride + source_area.x as usize,
                    ),
                    self.base
                        .add((target_top as usize + row) * self.stride + target_left as usize),
                    source_area.width as usize,
                );
            }
        }
    }

    /// 从另一块内存整行拷贝像素。
    ///
    /// # Safety
    ///
    /// `source` 必须指向至少 `count` 个可读像素。
    pub unsafe fn copy_row(&self, x: i32, y: i32, count: usize, source: *const u32) {
        let width = if count > i32::MAX as usize {
            i32::MAX
        } else {
            count as i32
        };
        let area = Rect::new(x, y, width, 1).intersect(self.clip);
        if area.is_empty() {
            return;
        }
        // SAFETY: 目标行在裁剪矩形内；源偏移由调用方保证可读。
        unsafe {
            core::ptr::copy_nonoverlapping(
                source.add((area.x - x) as usize),
                self.base.add(y as usize * self.stride + area.x as usize),
                area.width as usize,
            );
        }
    }
}

/// 按 `ratio/255` 把 `to` 混入 `from`。
pub const fn mix(from: u32, to: u32, ratio: u8) -> u32 {
    let ratio = ratio as u32;
    let inverse = 255 - ratio;
    let mask = 0xFF;
    let red = (((from >> 16) & mask) * inverse + ((to >> 16) & mask) * ratio) / 255;
    let green = (((from >> 8) & mask) * inverse + ((to >> 8) & mask) * ratio) / 255;
    let blue = ((from & mask) * inverse + (to & mask) * ratio) / 255;
    (red << 16) | (green << 8) | blue
}
