//! 表面拷贝：整块拷贝、最近邻缩放与半透明拷贝。

use crate::color::mix;
use crate::rect::Rect;

use super::Surface;

impl Surface<'_> {
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
        let count = source_area.width as usize;
        if count == self.stride
            && source.stride == self.stride
            && source_area.x == 0
            && target_left == 0
        {
            // SAFETY: 整行宽且跨度一致，源与目标区域连续，长度按交集高度。
            unsafe {
                core::ptr::copy_nonoverlapping(
                    source.base.add(source_area.y as usize * source.stride),
                    self.base.add(target_top as usize * self.stride),
                    source_area.height as usize * self.stride,
                );
            }
            return;
        }
        for row in 0..source_area.height as usize {
            // SAFETY: 源行与目标行都落在各自表面的裁剪矩形内，长度按交集宽度裁剪。
            unsafe {
                core::ptr::copy_nonoverlapping(
                    source.base.add(
                        (source_area.y as usize + row) * source.stride + source_area.x as usize,
                    ),
                    self.base
                        .add((target_top as usize + row) * self.stride + target_left as usize),
                    count,
                );
            }
        }
    }

    /// 把另一个表面按目标尺寸做最近邻缩放后拷贝到 `(at_x, at_y)`。
    pub fn blit_scaled(&self, at_x: i32, at_y: i32, width: i32, height: i32, source: &Surface<'_>) {
        if width <= 0 || height <= 0 || source.width == 0 || source.height == 0 {
            return;
        }
        let area = Rect::new(at_x, at_y, width, height).intersect(self.clip);
        if area.is_empty() {
            return;
        }
        let source_width = source.width as u64;
        let source_height = source.height as u64;
        let step_x = (source_width << 16) / width as u64;
        let step_y = (source_height << 16) / height as u64;
        let start_x = (((area.x - at_x) as u64 * source_width) << 16) / width as u64;
        let start_y = (((area.y - at_y) as u64 * source_height) << 16) / height as u64;
        let mut source_row = start_y;
        for row in area.y..area.bottom() {
            let source_y = ((source_row >> 16) as usize).min(source.height - 1);
            let mut source_column = start_x;
            // SAFETY: 行区间在裁剪矩形内，裁剪矩形在表面范围内。
            let pixel = unsafe { self.base.add(row as usize * self.stride + area.x as usize) };
            for column in area.x..area.right() {
                let source_x = ((source_column >> 16) as usize).min(source.width - 1);
                // SAFETY: 采样坐标按表面尺寸取整并夹取。
                let color = unsafe { source.base.add(source_y * source.stride + source_x).read() };
                // SAFETY: 目标坐标在裁剪矩形内。
                unsafe { pixel.add((column - area.x) as usize).write(color) };
                source_column = source_column.saturating_add(step_x);
            }
            source_row = source_row.saturating_add(step_y);
        }
    }

    /// 把另一个表面按 `ratio/255` 混合后拷贝到 `(at_x, at_y)`。
    pub fn blit_with_alpha(&self, at_x: i32, at_y: i32, source: &Surface<'_>, ratio: u8) {
        if ratio == 0 {
            return;
        }
        if ratio == 255 {
            self.blit(at_x, at_y, source);
            return;
        }
        let target = Rect::new(at_x, at_y, source.width as i32, source.height as i32);
        let area = target.intersect(self.clip);
        if area.is_empty() {
            return;
        }
        let source_area = Rect::new(
            area.x.saturating_sub(at_x),
            area.y.saturating_sub(at_y),
            area.width,
            area.height,
        )
        .intersect(source.clip);
        if source_area.is_empty() {
            return;
        }
        let target_left = source_area.x.saturating_add(at_x);
        let target_top = source_area.y.saturating_add(at_y);
        for row in 0..source_area.height as usize {
            let source_row = source_area.y as usize + row;
            let target_row = target_top as usize + row;
            // SAFETY: 源行与目标行的列区间都在各自裁剪矩形内。
            let mut source_pixel = unsafe {
                source
                    .base
                    .add(source_row * source.stride + source_area.x as usize)
            };
            // SAFETY: 目标行与源行同样落在表面范围内。
            let mut target_pixel = unsafe {
                self.base
                    .add(target_row * self.stride + target_left as usize)
            };
            for _ in 0..source_area.width as usize {
                // SAFETY: 源与目标坐标都在各自裁剪矩形内。
                unsafe {
                    let color = source_pixel.read();
                    target_pixel.write(mix(target_pixel.read(), color, ratio));
                    source_pixel = source_pixel.add(1);
                    target_pixel = target_pixel.add(1);
                }
            }
        }
    }
}
