//! 表面、裁剪与像素原语。

mod blit;
mod shapes;

use core::cell::Cell;
use core::marker::PhantomData;

use crate::color::mix;
use crate::rect::Rect;

/// 8.8 定点坐标中的一个整数像素。
const FP_ONE: i64 = 256;

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
    #[inline]
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
    #[inline]
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

    /// 用单色填充整块表面。
    pub fn clear(&self, color: u32) {
        self.fill(
            Rect::new(0, 0, self.width as i32, self.height as i32),
            color,
        );
    }

    /// 用单色填充一行。
    pub fn fill_row(&self, x: i32, y: i32, width: i32, color: u32) {
        self.fill(Rect::new(x, y, width, 1), color);
    }

    /// 按 `ratio/255` 把 `color` 混入一个像素；落在裁剪外时跳过。
    #[inline]
    pub fn blend(&self, x: i32, y: i32, color: u32, ratio: u8) {
        if ratio == 0 || !self.clip.contains(x, y) {
            return;
        }
        if ratio == 255 {
            self.put(x, y, color);
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
        if ratio == 0 {
            return;
        }
        let area = rect.intersect(self.clip);
        if area.is_empty() {
            return;
        }
        if ratio == 255 {
            self.fill(area, color);
            return;
        }
        for row in area.y..area.bottom() {
            // SAFETY: 行区间在裁剪矩形内，裁剪矩形在表面范围内。
            let pixels = unsafe {
                core::slice::from_raw_parts_mut(
                    self.base.add(row as usize * self.stride + area.x as usize),
                    area.width as usize,
                )
            };
            for pixel in pixels {
                *pixel = mix(*pixel, color, ratio);
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

    /// 把矩形内的像素整体平移 `(dx, dy)`，移出区域的内容被丢弃。
    ///
    /// 挪走内容后留下的空位保持原值，调用方按需回填背景。
    pub fn scroll(&self, rect: Rect, dx: i32, dy: i32) {
        if rect.is_empty() || (dx == 0 && dy == 0) {
            return;
        }
        let destination = rect
            .translate(dx, dy)
            .intersect(self.clip)
            .intersect(Rect::new(dx, dy, self.width as i32, self.height as i32));
        if destination.is_empty() {
            return;
        }
        let distance = (destination.right() - destination.x) as usize;
        let source_left = destination.x - dx;
        if dy >= 0 {
            for row in (destination.y..destination.bottom()).rev() {
                let source_row = row - dy;
                // SAFETY: 源与目标行都在矩形与表面范围内，`copy` 处理重叠。
                unsafe {
                    core::ptr::copy(
                        self.base
                            .add(source_row as usize * self.stride + source_left as usize),
                        self.base
                            .add(row as usize * self.stride + destination.x as usize),
                        distance,
                    );
                }
            }
        } else {
            for row in destination.y..destination.bottom() {
                let source_row = row - dy;
                // SAFETY: 源与目标行都在矩形与表面范围内，`copy` 处理重叠。
                unsafe {
                    core::ptr::copy(
                        self.base
                            .add(source_row as usize * self.stride + source_left as usize),
                        self.base
                            .add(row as usize * self.stride + destination.x as usize),
                        distance,
                    );
                }
            }
        }
    }

    /// 用固定点边界填充一行，边缘像素按覆盖率混入。
    fn fill_span(&self, left: i64, right: i64, row: i32, color: u32) {
        if right <= left {
            return;
        }
        let full_left = (left + FP_ONE - 1).div_euclid(FP_ONE) as i32;
        let full_right = right.div_euclid(FP_ONE) as i32;
        if full_left >= full_right {
            let pixel = left.div_euclid(FP_ONE) as i32;
            let coverage = (right - left).clamp(0, FP_ONE);
            self.blend(pixel, row, color, ratio_of(coverage));
            return;
        }
        let left_fraction = left.rem_euclid(FP_ONE);
        if left_fraction != 0 {
            self.blend(full_left - 1, row, color, ratio_of(FP_ONE - left_fraction));
        }
        self.fill(Rect::new(full_left, row, full_right - full_left, 1), color);
        let right_fraction = right.rem_euclid(FP_ONE);
        if right_fraction != 0 {
            self.blend(full_right, row, color, ratio_of(right_fraction));
        }
    }
}

/// 把覆盖率的 8.8 定点值换算成 0..=255 的混合比例。
fn ratio_of(fraction: i64) -> u8 {
    ((fraction.clamp(0, FP_ONE) * 255 + FP_ONE / 2) / FP_ONE) as u8
}
