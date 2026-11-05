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

#[cfg(test)]
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
}
