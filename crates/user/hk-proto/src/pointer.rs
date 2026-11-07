//! 指针光标形状。
//!
//! 光标形状由 Display 决定，客户端可以按表面请求：窗口正文里指针落在哪个表面
//! 上，就使用该表面声明的形状；窗口装饰与任务栏由 Display 自己选择。

/// 客户端可请求的光标形状。
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shape {
    /// 默认箭头。
    #[default]
    Arrow = 0,
    /// 可点击：按钮、任务栏按钮与链接。
    Hand = 1,
    /// 文本输入。
    Text = 2,
    /// 可移动。
    Move = 3,
    /// 左右缩放。
    ResizeHorizontal = 4,
    /// 上下缩放。
    ResizeVertical = 5,
    /// 左上到右下缩放。
    ResizeNwSe = 6,
    /// 右上到左下缩放。
    ResizeNeSw = 7,
}

impl Shape {
    /// 从线值解出形状；未定义值返回 `None`。
    pub const fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            0 => Self::Arrow,
            1 => Self::Hand,
            2 => Self::Text,
            3 => Self::Move,
            4 => Self::ResizeHorizontal,
            5 => Self::ResizeVertical,
            6 => Self::ResizeNwSe,
            7 => Self::ResizeNeSw,
            _ => return None,
        })
    }
}

/// 把表面编号与光标形状打包进一个消息字。
pub const fn pack_surface_cursor(surface: u64, shape: Shape) -> u64 {
    (surface << 8) | shape as u64
}

/// 从消息字解包表面编号与光标形状。
pub const fn unpack_surface_cursor(word: u64) -> (u64, Option<Shape>) {
    (word >> 8, Shape::from_u8(word as u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_surface_cursor() {
        for (surface, shape) in [
            (0, Shape::Arrow),
            (1, Shape::Hand),
            (42, Shape::ResizeNwSe),
            (u32::MAX as u64, Shape::Text),
        ] {
            assert_eq!(
                unpack_surface_cursor(pack_surface_cursor(surface, shape)),
                (surface, Some(shape))
            );
        }
    }

    #[test]
    fn rejects_unknown_shape() {
        assert_eq!(Shape::from_u8(8), None);
        assert_eq!(unpack_surface_cursor((7 << 8) | 8), (7, None));
    }
}
