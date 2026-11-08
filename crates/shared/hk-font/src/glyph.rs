//! 字形状态与对外视图。

use spin::MutexGuard;

/// 单字形位图边长上限。
pub(crate) const MAX_GLYPH: usize = 48;

/// 字形度量。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metric {
    /// 水平前进宽度（像素）。
    pub xadvance: i32,
}

/// 已栅格化的字形。
pub struct Glyph<'a> {
    pub(crate) state: MutexGuard<'a, State>,
}

impl Glyph<'_> {
    /// 遍历覆盖像素，坐标相对单元格左上角。
    pub fn for_each_pixel(&self, mut draw: impl FnMut(i32, i32)) {
        for row in 0..self.state.height {
            for column in 0..self.state.width {
                if self.state.buffer[row * MAX_GLYPH + column] >= 96 {
                    draw(self.state.left + column as i32, self.state.top + row as i32);
                }
            }
        }
    }

    /// 返回度量。
    pub fn metric(&self) -> Metric {
        Metric {
            xadvance: self.state.advance,
        }
    }

    /// 位图宽度（像素）。
    pub fn width(&self) -> usize {
        self.state.width
    }

    /// 位图高度（像素）。
    pub fn height(&self) -> usize {
        self.state.height
    }

    /// 位图左边相对绘制原点的偏移。
    pub fn left(&self) -> i32 {
        self.state.left
    }

    /// 位图顶边相对绘制原点的偏移。
    pub fn top(&self) -> i32 {
        self.state.top
    }

    /// 覆盖率数据的行跨度。
    pub fn stride(&self) -> usize {
        MAX_GLYPH
    }

    /// 覆盖率数据：0 为完全透明，255 为完全覆盖。
    pub fn coverage(&self) -> &[u8] {
        &self.state.buffer[..]
    }
}

/// 当前字形栅格化的可变状态。
pub(crate) struct State {
    pub(crate) buffer: [u8; MAX_GLYPH * MAX_GLYPH],
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) left: i32,
    pub(crate) top: i32,
    pub(crate) advance: i32,
}

impl State {
    pub(crate) const fn new() -> Self {
        Self {
            buffer: [0; MAX_GLYPH * MAX_GLYPH],
            width: 0,
            height: 0,
            left: 0,
            top: 0,
            advance: 0,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.width = 0;
        self.height = 0;
        self.left = 0;
        self.top = 0;
        self.advance = 0;
    }
}
