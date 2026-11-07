//! 整数矩形与集合运算。

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

    /// 平移矩形。
    pub const fn translate(self, dx: i32, dy: i32) -> Rect {
        Rect {
            x: self.x.saturating_add(dx),
            y: self.y.saturating_add(dy),
            width: self.width,
            height: self.height,
        }
    }

    /// 向四周扩张 `dx` 列、`dy` 行（负值为收缩）。
    pub const fn inflate(self, dx: i32, dy: i32) -> Rect {
        Rect {
            x: self.x.saturating_sub(dx),
            y: self.y.saturating_sub(dy),
            width: self.width.saturating_add(dx.saturating_mul(2)),
            height: self.height.saturating_add(dy.saturating_mul(2)),
        }
    }

    /// 包含两个矩形的最小矩形；空矩形不参与包围。
    pub const fn union(self, other: Rect) -> Rect {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        let x = if self.x < other.x { self.x } else { other.x };
        let y = if self.y < other.y { self.y } else { other.y };
        let right = if self.right() > other.right() {
            self.right()
        } else {
            other.right()
        };
        let bottom = if self.bottom() > other.bottom() {
            self.bottom()
        } else {
            other.bottom()
        };
        Rect::new(x, y, right - x, bottom - y)
    }
}
