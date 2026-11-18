//! 窗口缩放几何。

/// 窗口矩形（不含标题栏）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Rect {
    pub(crate) x: usize,
    pub(crate) y: usize,
    pub(crate) width: usize,
    pub(crate) height: usize,
}

/// 缩放命中的窗口边。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Edges {
    pub(crate) left: bool,
    pub(crate) right: bool,
    pub(crate) top: bool,
    pub(crate) bottom: bool,
}

impl Edges {
    /// 是否命中至少一条边。
    #[cfg(target_os = "none")]
    pub(crate) const fn any(self) -> bool {
        self.left || self.right || self.top || self.bottom
    }
}

/// 根据拖动增量计算新矩形。
///
/// 左侧/顶部缩放保持右边/底边固定，右侧/底部缩放保持左上角固定；结果同时
/// 满足最小尺寸、屏幕左右边界与任务栏上方边界。
#[allow(clippy::too_many_arguments)]
pub(crate) fn resized(
    original: Rect,
    edges: Edges,
    delta_x: isize,
    delta_y: isize,
    screen_width: usize,
    desktop_height: usize,
    title_height: usize,
    min_width: usize,
    min_height: usize,
) -> Rect {
    let mut rect = original;
    if edges.left {
        let fixed_right = original.x.saturating_add(original.width);
        let max_left = fixed_right.saturating_sub(min_width);
        rect.x = (original.x as isize + delta_x).clamp(0, max_left as isize) as usize;
        rect.width = fixed_right.saturating_sub(rect.x);
    } else if edges.right {
        let max_width = screen_width.saturating_sub(original.x).max(min_width);
        rect.width = (original.width as isize + delta_x)
            .clamp(min_width as isize, max_width as isize) as usize;
    }
    if edges.top {
        let fixed_bottom = original.y.saturating_add(original.height);
        let max_top = fixed_bottom.saturating_sub(min_height);
        rect.y = (original.y as isize + delta_y).clamp(0, max_top as isize) as usize;
        rect.height = fixed_bottom.saturating_sub(rect.y);
    } else if edges.bottom {
        let max_height = desktop_height
            .saturating_sub(original.y + title_height)
            .max(min_height);
        rect.height = (original.height as isize + delta_y)
            .clamp(min_height as isize, max_height as isize) as usize;
    }
    rect
}

/// 拖拽后的窗口原点：按指针偏移并夹取在桌面内。
pub(crate) fn dragged(
    pointer: (usize, usize),
    offset: (usize, usize),
    width: usize,
    height: usize,
    desktop: (usize, usize),
    title_height: usize,
) -> (usize, usize) {
    let max_x = desktop.0.saturating_sub(width);
    let max_y = desktop.1.saturating_sub(height + title_height);
    (
        pointer.0.saturating_sub(offset.0).min(max_x),
        pointer.1.saturating_sub(offset.1).min(max_y),
    )
}

/// 矩形向外扩 `margin`。
fn expand(rect: Rect, margin: usize) -> Rect {
    Rect {
        x: rect.x.saturating_sub(margin),
        y: rect.y.saturating_sub(margin),
        width: rect.width.saturating_add(margin * 2),
        height: rect.height.saturating_add(margin * 2),
    }
}

/// 只保留非空矩形的构造。
fn band(x: usize, y: usize, width: usize, height: usize) -> Option<Rect> {
    (width > 0 && height > 0).then_some(Rect {
        x,
        y,
        width,
        height,
    })
}

/// `rect` 去掉 `hole` 后剩下的边带：上半、下半、中间左侧、中间右侧。
fn subtract(rect: Rect, hole: Rect) -> [Option<Rect>; 4] {
    let right = rect.x.saturating_add(rect.width);
    let bottom = rect.y.saturating_add(rect.height);
    let hole_right = hole.x.saturating_add(hole.width);
    let hole_bottom = hole.y.saturating_add(hole.height);

    let top_end = hole.y.min(bottom);
    let bottom_start = hole_bottom.max(rect.y).min(bottom);
    let middle_top = hole.y.max(rect.y);
    let middle_bottom = hole_bottom.min(bottom);
    let mut sides = [None; 2];
    if middle_top < middle_bottom {
        let left_end = hole.x.min(right);
        sides[0] = band(
            rect.x,
            middle_top,
            left_end.saturating_sub(rect.x),
            middle_bottom - middle_top,
        );
        let right_start = hole_right.max(rect.x).min(right);
        sides[1] = band(
            right_start,
            middle_top,
            right.saturating_sub(right_start),
            middle_bottom - middle_top,
        );
    }
    [
        band(rect.x, rect.y, rect.width, top_end.saturating_sub(rect.y)),
        band(
            rect.x,
            bottom_start,
            rect.width,
            bottom.saturating_sub(bottom_start),
        ),
        sides[0],
        sides[1],
    ]
}

/// 内沿四周的投影环：上、下、左、右四条边带。
fn ring(inner: Rect, margin: usize) -> [Option<Rect>; 4] {
    let outer = expand(inner, margin);
    let right = outer.x.saturating_add(outer.width);
    let bottom = outer.y.saturating_add(outer.height);
    let inner_right = inner.x.saturating_add(inner.width);
    let inner_bottom = inner.y.saturating_add(inner.height);
    [
        band(
            outer.x,
            outer.y,
            outer.width,
            inner.y.saturating_sub(outer.y),
        ),
        band(
            outer.x,
            inner_bottom,
            outer.width,
            bottom.saturating_sub(inner_bottom),
        ),
        band(
            outer.x,
            inner.y,
            inner.x.saturating_sub(outer.x),
            inner.height,
        ),
        band(
            inner_right,
            inner.y,
            right.saturating_sub(inner_right),
            inner.height,
        ),
    ]
}

/// 窗口平移后需要重绘的区域：旧外沿去掉新内沿，加上新内沿外的投影环。
///
/// 窗口内容已作为像素整体搬走后，这些边带是唯一还带着旧画面或缺少投影的
/// 地方；合成它们即可得到与整窗重绘相同的画面。
pub(crate) fn move_repair(old: Rect, new: Rect, margin: usize) -> [Option<Rect>; 8] {
    let exposed = subtract(expand(old, margin), new);
    let ring = ring(new, margin);
    [
        exposed[0], exposed[1], exposed[2], exposed[3], ring[0], ring[1], ring[2], ring[3],
    ]
}

#[cfg(test)]
#[allow(clippy::wildcard_imports)]
mod tests {
    use super::*;

    const MIN_WIDTH: usize = 120;
    const MIN_HEIGHT: usize = 80;
    const SCREEN_WIDTH: usize = 1280;
    const DESKTOP_HEIGHT: usize = 772;
    const TITLE_HEIGHT: usize = 24;

    fn base() -> Rect {
        Rect {
            x: 440,
            y: 224,
            width: 400,
            height: 300,
        }
    }

    fn apply(edges: Edges, dx: isize, dy: isize) -> Rect {
        resized(
            base(),
            edges,
            dx,
            dy,
            SCREEN_WIDTH,
            DESKTOP_HEIGHT,
            TITLE_HEIGHT,
            MIN_WIDTH,
            MIN_HEIGHT,
        )
    }

    #[test]
    fn grows_from_right_and_bottom() {
        let rect = apply(
            Edges {
                right: true,
                bottom: true,
                ..Edges::default()
            },
            64,
            48,
        );
        assert_eq!(
            rect,
            Rect {
                x: 440,
                y: 224,
                width: 464,
                height: 348,
            }
        );
    }

    #[test]
    fn left_and_top_keep_opposite_edges_fixed() {
        let rect = apply(
            Edges {
                left: true,
                top: true,
                ..Edges::default()
            },
            -20,
            -20,
        );
        assert_eq!(
            rect,
            Rect {
                x: 420,
                y: 204,
                width: 420,
                height: 320,
            }
        );
    }

    #[test]
    fn clamps_to_minimum_size() {
        let rect = apply(
            Edges {
                right: true,
                bottom: true,
                ..Edges::default()
            },
            -10_000,
            -10_000,
        );
        assert_eq!(rect.width, MIN_WIDTH);
        assert_eq!(rect.height, MIN_HEIGHT);
    }

    #[test]
    fn left_and_top_stop_at_screen_origin() {
        let rect = apply(
            Edges {
                left: true,
                top: true,
                ..Edges::default()
            },
            -10_000,
            -10_000,
        );
        assert_eq!(rect.x, 0);
        assert_eq!(rect.y, 0);
        assert_eq!(rect.width, 840);
        assert_eq!(rect.height, 524);
    }

    #[test]
    fn bottom_stops_at_taskbar() {
        let rect = apply(
            Edges {
                bottom: true,
                ..Edges::default()
            },
            0,
            10_000,
        );
        assert_eq!(rect.height, DESKTOP_HEIGHT - base().y - TITLE_HEIGHT);
    }

    #[test]
    fn drag_follows_pointer_with_offset() {
        assert_eq!(
            dragged((640, 400), (100, 30), 400, 300, (1280, 772), 24),
            (540, 370)
        );
    }

    #[test]
    fn drag_clamps_to_desktop() {
        assert_eq!(
            dragged((5000, 5000), (0, 0), 400, 300, (1280, 772), 24),
            (880, 448)
        );
        assert_eq!(
            dragged((10, 10), (20, 20), 400, 300, (1280, 772), 24),
            (0, 0)
        );
    }

    fn inside(rect: Rect, x: usize, y: usize) -> bool {
        x >= rect.x
            && y >= rect.y
            && x < rect.x.saturating_add(rect.width)
            && y < rect.y.saturating_add(rect.height)
    }

    fn check_repair(old: Rect, new: Rect, margin: usize, limit: (usize, usize)) {
        let rects = move_repair(old, new, margin);
        let old_outer = expand(old, margin);
        let new_outer = expand(new, margin);
        for y in 0..limit.1 {
            for x in 0..limit.0 {
                let stale =
                    (inside(old_outer, x, y) || inside(new_outer, x, y)) && !inside(new, x, y);
                let covered = rects.iter().flatten().any(|rect| inside(*rect, x, y));
                assert_eq!(covered, stale, "({x},{y}) 覆盖与预期不符");
            }
        }
    }

    #[test]
    fn move_repair_covers_stale_bands() {
        check_repair(
            Rect {
                x: 100,
                y: 80,
                width: 200,
                height: 150,
            },
            Rect {
                x: 111,
                y: 74,
                width: 200,
                height: 150,
            },
            4,
            (420, 400),
        );
    }

    #[test]
    fn move_repair_handles_disjoint_positions() {
        check_repair(
            Rect {
                x: 10,
                y: 10,
                width: 40,
                height: 30,
            },
            Rect {
                x: 300,
                y: 200,
                width: 40,
                height: 30,
            },
            2,
            (360, 250),
        );
    }
}
