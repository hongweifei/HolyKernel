//! 表面：客户端缓冲、damage 累积与输入区域。
//!
//! 表面是 Display 的合成单元；窗口只负责几何、装饰与 z 序。客户端提交的
//! damage 与输入区域都在表面缓冲坐标系中记录，合成时再映射到窗口坐标。

use crate::resize::Rect;

/// 同时存在的表面上限。
#[cfg_attr(test, allow(dead_code))]
pub(crate) const MAX_SURFACES: usize = 8;

/// 客户端提交的缓冲与表面状态。
#[cfg_attr(test, allow(dead_code))]
#[derive(Clone, Copy, Debug)]
pub(crate) struct Surface {
    pub(crate) id: u64,
    pub(crate) client: u64,
    pub(crate) slot: usize,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) virt: u64,
    pub(crate) region: u64,
    pub(crate) handle: u64,
    /// 待提交的 damage 区域；`None` 表示整面。
    damage: Option<Rect>,
    /// 输入区域；`None` 表示整个表面。
    input: Option<Rect>,
}

impl Surface {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        id: u64,
        client: u64,
        slot: usize,
        width: usize,
        height: usize,
        virt: u64,
        region: u64,
        handle: u64,
    ) -> Self {
        Self {
            id,
            client,
            slot,
            width,
            height,
            virt,
            region,
            handle,
            damage: None,
            input: None,
        }
    }

    /// 记录一块 damage，裁剪到表面边界后与已有区域合并。
    pub(crate) fn mark_damage(&mut self, x: usize, y: usize, width: usize, height: usize) {
        let Some(rect) = clip(x, y, width, height, self.width, self.height) else {
            return;
        };
        self.damage = Some(match self.damage {
            Some(current) => union(current, rect),
            None => rect,
        });
    }

    /// 取出并清空待提交的 damage。
    pub(crate) fn take_damage(&mut self) -> Option<Rect> {
        self.damage.take()
    }

    /// 设置输入区域；空矩形恢复为整个表面。
    pub(crate) fn set_input_region(&mut self, rect: Rect) {
        self.input = clip(
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            self.width,
            self.height,
        )
        .filter(|rect| rect.width > 0 && rect.height > 0);
    }

    /// 表面坐标是否落在输入区域内。
    pub(crate) fn input_contains(&self, x: usize, y: usize) -> bool {
        match self.input {
            Some(rect) => {
                x >= rect.x
                    && y >= rect.y
                    && x < rect.x.saturating_add(rect.width)
                    && y < rect.y.saturating_add(rect.height)
            }
            None => x < self.width && y < self.height,
        }
    }
}

/// 把矩形裁剪到给定边界；完全越界时返回 `None`。
pub(crate) fn clip(
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    limit_width: usize,
    limit_height: usize,
) -> Option<Rect> {
    let left = x.min(limit_width);
    let top = y.min(limit_height);
    let right = x.saturating_add(width).min(limit_width);
    let bottom = y.saturating_add(height).min(limit_height);
    if left >= right || top >= bottom {
        return None;
    }
    Some(Rect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    })
}

/// 两个矩形的外接矩形。
pub(crate) fn union(current: Rect, next: Rect) -> Rect {
    let left = current.x.min(next.x);
    let top = current.y.min(next.y);
    let right = current
        .x
        .saturating_add(current.width)
        .max(next.x.saturating_add(next.width));
    let bottom = current
        .y
        .saturating_add(current.height)
        .max(next.y.saturating_add(next.height));
    Rect {
        x: left,
        y: top,
        width: right.saturating_sub(left),
        height: bottom.saturating_sub(top),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface(width: usize, height: usize) -> Surface {
        Surface::new(1, 0, 0, width, height, 0, 0, 0)
    }

    #[test]
    fn damage_merges_and_clips() {
        let mut surface = surface(100, 50);
        surface.mark_damage(10, 10, 20, 20);
        surface.mark_damage(40, 5, 200, 10);
        assert_eq!(
            surface.take_damage(),
            Some(Rect {
                x: 10,
                y: 5,
                width: 90,
                height: 25,
            })
        );
        assert_eq!(surface.take_damage(), None);
    }

    #[test]
    fn empty_damage_is_ignored() {
        let mut surface = surface(10, 10);
        surface.mark_damage(20, 20, 5, 5);
        assert_eq!(surface.take_damage(), None);
    }

    #[test]
    fn input_region_defaults_to_whole_surface() {
        let surface = surface(10, 20);
        assert!(surface.input_contains(0, 0));
        assert!(surface.input_contains(9, 19));
        assert!(!surface.input_contains(10, 0));
    }

    #[test]
    fn input_region_limits_hits() {
        let mut surface = surface(100, 100);
        surface.set_input_region(Rect {
            x: 10,
            y: 20,
            width: 30,
            height: 40,
        });
        assert!(surface.input_contains(10, 20));
        assert!(surface.input_contains(39, 59));
        assert!(!surface.input_contains(9, 20));
        assert!(!surface.input_contains(40, 60));
    }

    #[test]
    fn empty_input_region_restores_whole_surface() {
        let mut surface = surface(100, 100);
        surface.set_input_region(Rect::default());
        assert!(surface.input_contains(99, 99));
    }
}
