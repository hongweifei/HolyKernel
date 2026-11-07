//! 指针系统：坐标映射、悬停命中、光标形状、双击判定与客户端指针事件。
//!
//! 指针位置由输入驱动按 0..=0x7FFF 的绝对坐标上报，Display 归一化到屏幕像素，
//! 再按 z 序命中窗口装饰、任务栏或窗口正文。悬停决定光标形状与按钮高亮；正文
//! 移动同时按窗口路由给客户端。

#![cfg_attr(not(target_os = "none"), allow(dead_code))]

use hk_proto::pointer::Shape;

use crate::resize::Edges;

/// 双击判定的时间窗口（微秒）。
pub(crate) const DOUBLE_CLICK_MICROS: u64 = 400_000;

/// 指针命中类别；决定光标形状与悬停高亮。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hover {
    /// 桌面或任务栏空白处。
    None,
    /// 任务栏上的窗口按钮。
    Taskbar(u64),
    /// 标题栏关闭按钮。
    Close(u64),
    /// 标题栏最小化按钮。
    Minimize(u64),
    /// 标题栏最大化/还原按钮。
    Maximize(u64),
    /// 标题栏正文。
    Title(u64),
    /// 窗口缩放边。
    Resize(u64, Edges),
    /// 窗口正文；客户端可声明光标形状。
    Body(u64),
}

/// 要绘制的光标快照。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cursor {
    pub(crate) x: usize,
    pub(crate) y: usize,
    pub(crate) shape: Shape,
}

/// 指针状态。
pub(crate) struct Pointer {
    position: Option<(usize, usize)>,
    shape: Shape,
    pressed: bool,
    last_press: Option<(Hover, u64)>,
}

impl Pointer {
    pub(crate) const fn new() -> Self {
        Self {
            position: None,
            shape: Shape::Arrow,
            pressed: false,
            last_press: None,
        }
    }

    /// 当前屏幕坐标；指针还未上报过时为 `None`。
    pub(crate) fn position(&self) -> Option<(usize, usize)> {
        self.position
    }

    /// 当前光标快照。
    pub(crate) fn cursor(&self) -> Option<Cursor> {
        self.position.map(|(x, y)| Cursor {
            x,
            y,
            shape: self.shape,
        })
    }

    /// 记录指针位置。
    pub(crate) fn set_position(&mut self, position: (usize, usize)) {
        self.position = Some(position);
    }

    /// 更新光标形状；返回形状是否发生变化。
    pub(crate) fn set_shape(&mut self, shape: Shape) -> bool {
        if self.shape == shape {
            return false;
        }
        self.shape = shape;
        true
    }

    /// 按下状态。
    pub(crate) fn pressed(&self) -> bool {
        self.pressed
    }

    /// 记录按下或释放。
    pub(crate) fn set_pressed(&mut self, pressed: bool) {
        self.pressed = pressed;
    }

    /// 记录一次按下；同一目标在双击窗口内再次按下时返回 2。
    pub(crate) fn register_press(&mut self, hover: Hover, now_ticks: u64, frequency: u64) -> u8 {
        let interval = double_click_interval(frequency);
        match self.last_press {
            Some((target, last))
                if target == hover && now_ticks.saturating_sub(last) <= interval =>
            {
                self.last_press = None;
                2
            }
            _ => {
                self.last_press = Some((hover, now_ticks));
                1
            }
        }
    }
}

/// 把输入驱动的绝对坐标映射到屏幕像素范围。
pub(crate) fn normalize(x: u16, y: u16, width: usize, height: usize) -> (usize, usize) {
    let limit_x = width.saturating_sub(1) as u64;
    let limit_y = height.saturating_sub(1) as u64;
    (
        (u64::from(x) * width as u64 / 0x7FFF).min(limit_x) as usize,
        (u64::from(y) * height as u64 / 0x7FFF).min(limit_y) as usize,
    )
}

/// 缩放边对应的光标形状。
pub(crate) fn shape_for_edges(edges: Edges) -> Shape {
    if (edges.left && edges.top) || (edges.right && edges.bottom) {
        Shape::ResizeNwSe
    } else if (edges.right && edges.top) || (edges.left && edges.bottom) {
        Shape::ResizeNeSw
    } else if edges.left || edges.right {
        Shape::ResizeHorizontal
    } else if edges.top || edges.bottom {
        Shape::ResizeVertical
    } else {
        Shape::Arrow
    }
}

/// 光标形状的日志标签。
pub(crate) fn label(shape: Shape) -> &'static str {
    match shape {
        Shape::Arrow => "arrow",
        Shape::Hand => "hand",
        Shape::Text => "text",
        Shape::Move => "move",
        Shape::ResizeHorizontal => "resize-h",
        Shape::ResizeVertical => "resize-v",
        Shape::ResizeNwSe => "resize-nwse",
        Shape::ResizeNeSw => "resize-nesw",
    }
}

fn double_click_interval(frequency: u64) -> u64 {
    frequency.saturating_mul(DOUBLE_CLICK_MICROS) / 1_000_000
}

/// 光标位图；`#` 为不透明像素，其余为透明。
pub(crate) fn art(shape: Shape) -> &'static [&'static str] {
    match shape {
        Shape::Arrow => ARROW,
        Shape::Hand => HAND,
        Shape::Text => TEXT,
        Shape::Move => MOVE,
        Shape::ResizeHorizontal => RESIZE_HORIZONTAL,
        Shape::ResizeVertical => RESIZE_VERTICAL,
        Shape::ResizeNwSe => RESIZE_NW_SE,
        Shape::ResizeNeSw => RESIZE_NE_SW,
    }
}

fn art_size(rows: &[&str]) -> (usize, usize) {
    (rows.first().map_or(0, |row| row.len()), rows.len())
}

fn hotspot(shape: Shape, width: usize, height: usize) -> (usize, usize) {
    match shape {
        Shape::Arrow | Shape::Hand => (0, 0),
        _ => (width / 2, height / 2),
    }
}

fn clamp_origin(value: usize, extent: usize, limit: usize) -> usize {
    value.min(limit.saturating_sub(extent))
}

/// 光标覆盖的屏幕矩形（含投影）；用于合成与 damage。
pub(crate) fn cursor_rect(
    cursor: Cursor,
    screen_width: usize,
    screen_height: usize,
    scale: usize,
) -> (usize, usize, usize, usize) {
    let scale = scale.max(1);
    let (width, height) = art_size(art(cursor.shape));
    let (hot_x, hot_y) = hotspot(cursor.shape, width, height);
    let extent_x = width * scale + scale;
    let extent_y = height * scale + scale;
    let x = clamp_origin(
        cursor.x.saturating_sub(hot_x * scale),
        extent_x,
        screen_width,
    );
    let y = clamp_origin(
        cursor.y.saturating_sub(hot_y * scale),
        extent_y,
        screen_height,
    );
    (x, y, extent_x, extent_y)
}

/// 在屏幕上绘制光标：先画向右下偏移的投影，再画本体。
///
/// 每行按连续不透明像素合并为一段，一次填充整段，避免逐像素调用。
#[cfg(target_os = "none")]
pub(crate) fn paint_cursor(screen: &crate::window::Screen, cursor: Cursor) {
    let scale = crate::theme::scale();
    let palette = crate::theme::theme();
    let rows = art(cursor.shape);
    let (x, y, _, _) = cursor_rect(cursor, screen.width, screen.height, scale);
    fill_spans(
        screen,
        rows,
        x + scale,
        y + scale,
        scale,
        palette.cursor_shadow,
    );
    fill_spans(screen, rows, x, y, scale, palette.cursor);
}

/// 按连续 `#` 段把光标位图放大 `scale` 倍后填充到屏幕。
#[cfg(target_os = "none")]
fn fill_spans(
    screen: &crate::window::Screen,
    rows: &[&str],
    x: usize,
    y: usize,
    scale: usize,
    color: u32,
) {
    for (row, line) in rows.iter().enumerate() {
        let bytes = line.as_bytes();
        let mut column = 0;
        while column < bytes.len() {
            if bytes[column] != b'#' {
                column += 1;
                continue;
            }
            let start = column;
            while column < bytes.len() && bytes[column] == b'#' {
                column += 1;
            }
            let pixel_x = x + start * scale;
            let pixel_y = y + row * scale;
            for offset in 0..scale {
                screen.fill_row(pixel_x, pixel_y + offset, (column - start) * scale, color);
            }
        }
    }
}

/// 按 z 序命中指针位置。
#[cfg(target_os = "none")]
pub(crate) fn hover_at(
    windows: &[Option<crate::window::Window>; crate::window::MAX_WINDOWS],
    screen: &crate::window::Screen,
    x: usize,
    y: usize,
) -> Hover {
    use crate::window::{
        close_at, maximize_at, minimize_at, resize_at, taskbar_at, taskbar_top, title_height,
        window_at,
    };

    if y >= taskbar_top(screen.height) {
        return taskbar_at(windows, x, y, screen.width, screen.height)
            .map(Hover::Taskbar)
            .unwrap_or(Hover::None);
    }
    let Some(window) = window_at(windows, x, y) else {
        return Hover::None;
    };
    let id = window.id;
    if close_at(&window, x, y) {
        return Hover::Close(id);
    }
    if minimize_at(&window, x, y) {
        return Hover::Minimize(id);
    }
    if maximize_at(&window, x, y) {
        return Hover::Maximize(id);
    }
    if let Some(edges) = resize_at(&window, x, y) {
        return Hover::Resize(id, edges);
    }
    if y < window.y.saturating_add(title_height()) {
        return Hover::Title(id);
    }
    Hover::Body(id)
}

/// 悬停对应的光标形状；正文使用表面声明的形状。
#[cfg(target_os = "none")]
pub(crate) fn shape_for_hover(
    hover: Hover,
    windows: &[Option<crate::window::Window>; crate::window::MAX_WINDOWS],
) -> Shape {
    match hover {
        Hover::None => Shape::Arrow,
        Hover::Body(id) => windows
            .iter()
            .flatten()
            .find(|window| window.id == id)
            .map(|window| window.surface.cursor)
            .unwrap_or(Shape::Arrow),
        Hover::Taskbar(_) | Hover::Close(_) | Hover::Minimize(_) | Hover::Maximize(_) => {
            Shape::Hand
        }
        Hover::Title(_) => Shape::Move,
        Hover::Resize(_, edges) => shape_for_edges(edges),
    }
}

/// 悬停目标对应的可见区域；只有按钮与任务栏需要重绘高亮。
#[cfg(target_os = "none")]
pub(crate) fn hover_rect(
    windows: &[Option<crate::window::Window>; crate::window::MAX_WINDOWS],
    screen: &crate::window::Screen,
    hover: Option<Hover>,
) -> Option<(usize, usize, usize, usize)> {
    use crate::window::{
        close_button_origin, close_size, maximize_button_origin, minimize_button_origin,
        taskbar_button_height, taskbar_button_top, taskbar_button_width, taskbar_button_x,
    };

    let hover = hover?;
    match hover {
        Hover::Close(id) | Hover::Minimize(id) | Hover::Maximize(id) => {
            let window = windows.iter().flatten().find(|window| window.id == id)?;
            let (x, y) = match hover {
                Hover::Close(_) => close_button_origin(window),
                Hover::Minimize(_) => minimize_button_origin(window),
                Hover::Maximize(_) => maximize_button_origin(window),
                _ => return None,
            };
            let size = close_size();
            let pad = crate::theme::scale();
            Some((
                x.saturating_sub(pad),
                y.saturating_sub(pad),
                size + pad * 2,
                size + pad * 2,
            ))
        }
        Hover::Taskbar(id) => {
            let index = windows
                .iter()
                .flatten()
                .position(|window| window.id == id)?;
            let x = taskbar_button_x(index);
            let width = taskbar_button_width().min(screen.width.saturating_sub(x));
            Some((
                x,
                taskbar_button_top(screen.height),
                width,
                taskbar_button_height(),
            ))
        }
        _ => None,
    }
}

/// 把正文指针位置与进出事件发给窗口客户端。
#[cfg(target_os = "none")]
pub(crate) fn route_pointer(
    windows: &[Option<crate::window::Window>; crate::window::MAX_WINDOWS],
    target: &mut Option<u64>,
    last: &mut Option<(u64, u32, u32)>,
    hover: Hover,
    x: usize,
    y: usize,
) {
    use crate::window::title_height;
    use hk_proto::display::{DISPLAY_POINTER, DISPLAY_POINTER_LEAVE, pack_click_at};

    let body = match hover {
        Hover::Body(id) => Some(id),
        _ => None,
    };
    if *target != body {
        if let Some(previous) = target.take()
            && let Some(window) = windows
                .iter()
                .flatten()
                .find(|window| window.id == previous)
        {
            let _ = hk_server_api::send(window.surface.client, DISPLAY_POINTER_LEAVE, previous);
        }
        *target = body;
        *last = None;
    }
    let Some(id) = body else {
        return;
    };
    let Some(window) = windows.iter().flatten().find(|window| window.id == id) else {
        return;
    };
    let body_x = x.saturating_sub(window.x) as u32;
    let body_y = y.saturating_sub(window.y + title_height()) as u32;
    if *last == Some((id, body_x, body_y)) {
        return;
    }
    *last = Some((id, body_x, body_y));
    let _ = hk_server_api::send(
        window.surface.client,
        DISPLAY_POINTER,
        pack_click_at(id, body_x, body_y),
    );
}

/// 设置表面请求的光标形状。
#[cfg(target_os = "none")]
pub(crate) fn set_surface_cursor(
    windows: &mut [Option<crate::window::Window>; crate::window::MAX_WINDOWS],
    surfaces: &mut [Option<crate::surface::Surface>; crate::surface::MAX_SURFACES],
    surface_id: u64,
    shape: Shape,
) -> bool {
    if let Some(window) = windows
        .iter_mut()
        .flatten()
        .find(|window| window.surface.id == surface_id)
    {
        window.surface.cursor = shape;
        return true;
    }
    if let Some(surface) = surfaces
        .iter_mut()
        .flatten()
        .find(|surface| surface.id == surface_id)
    {
        surface.cursor = shape;
        return true;
    }
    false
}

const ARROW: &[&str] = &[
    "#.......", "##......", "###.....", "####....", "#####...", "######..", "#######.", "########",
    "####....", "##..#...", "#...##..", "....##..",
];

const HAND: &[&str] = &[
    "....##.........",
    "...####........",
    "...#####.......",
    "...#####.......",
    "...#####.......",
    "...#####.......",
    "...#####.......",
    "##.#####.......",
    "##.#####.##....",
    "##.#####.##....",
    "##########.....",
    "###########....",
    ".##########....",
    "..#########....",
    "...#######.....",
    "....#####......",
];

const TEXT: &[&str] = &[
    "..###..", "...#...", "...#...", "...#...", "...#...", "...#...", "...#...", "...#...",
    "...#...", "...#...", "...#...", "...#...", "...#...", "...#...", "..###..",
];

const MOVE: &[&str] = &[
    "......####......",
    ".....######.....",
    "....########....",
    "......####......",
    "......####......",
    "......####......",
    ".#....####....#.",
    "###...####...###",
    "################",
    "################",
    "###...####...###",
    ".#....####....#.",
    "......####......",
    "......####......",
    "....########....",
    ".....######.....",
];

const RESIZE_HORIZONTAL: &[&str] = &[
    "................",
    "...#........#...",
    "..##........##..",
    ".###........###.",
    "################",
    ".###........###.",
    "..##........##..",
    "...#........#...",
    "................",
];

const RESIZE_VERTICAL: &[&str] = &[
    "....#....",
    "...###...",
    "..#####..",
    ".#######.",
    "....#....",
    "....#....",
    "....#....",
    "....#....",
    "....#....",
    "....#....",
    "....#....",
    "....#....",
    ".#######.",
    "..#####..",
    "...###...",
    "....#....",
];

const RESIZE_NW_SE: &[&str] = &[
    "##...........",
    "###..........",
    "####.........",
    ".###.........",
    "..###........",
    "...###.......",
    "....###......",
    ".....###.....",
    "......###....",
    ".......###...",
    "........###..",
    ".........####",
    "..........###",
];

const RESIZE_NE_SW: &[&str] = &[
    "...........##",
    "..........###",
    ".........####",
    ".........###.",
    "........###..",
    ".......###...",
    "......###....",
    ".....###.....",
    "....###......",
    "...###.......",
    "..###........",
    "####.........",
    "###..........",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn edges(left: bool, right: bool, top: bool, bottom: bool) -> Edges {
        Edges {
            left,
            right,
            top,
            bottom,
        }
    }

    #[test]
    fn normalizes_absolute_coordinates_to_last_pixel() {
        assert_eq!(normalize(0, 0, 1280, 800), (0, 0));
        assert_eq!(normalize(0x7FFF, 0x7FFF, 1280, 800), (1279, 799));
        assert_eq!(normalize(0x3FFF, 0x3FFF, 1280, 800), (639, 399));
    }

    #[test]
    fn zero_sized_screen_falls_back_to_origin() {
        assert_eq!(normalize(0x7FFF, 0x7FFF, 0, 0), (0, 0));
    }

    #[test]
    fn picks_cursor_shape_for_each_edge() {
        assert_eq!(
            shape_for_edges(edges(false, true, false, false)),
            Shape::ResizeHorizontal
        );
        assert_eq!(
            shape_for_edges(edges(false, false, false, true)),
            Shape::ResizeVertical
        );
        assert_eq!(
            shape_for_edges(edges(false, true, false, true)),
            Shape::ResizeNwSe
        );
        assert_eq!(
            shape_for_edges(edges(false, true, true, false)),
            Shape::ResizeNeSw
        );
        assert_eq!(
            shape_for_edges(edges(false, false, false, false)),
            Shape::Arrow
        );
    }

    #[test]
    fn registers_double_click_within_window() {
        let mut pointer = Pointer::new();
        let target = Hover::Title(1);
        assert_eq!(pointer.register_press(target, 1_000, 1_000_000), 1);
        assert_eq!(pointer.register_press(target, 200_000, 1_000_000), 2);
        assert_eq!(pointer.register_press(target, 201_000, 1_000_000), 1);
    }

    #[test]
    fn ignores_double_click_on_different_target_or_late_press() {
        let mut pointer = Pointer::new();
        assert_eq!(pointer.register_press(Hover::Title(1), 0, 1_000_000), 1);
        assert_eq!(
            pointer.register_press(Hover::Title(2), 10_000, 1_000_000),
            1
        );
        assert_eq!(
            pointer.register_press(Hover::Title(2), 900_000, 1_000_000),
            1
        );
    }

    #[test]
    fn cursor_bitmaps_are_rectangular() {
        for shape in [
            Shape::Arrow,
            Shape::Hand,
            Shape::Text,
            Shape::Move,
            Shape::ResizeHorizontal,
            Shape::ResizeVertical,
            Shape::ResizeNwSe,
            Shape::ResizeNeSw,
        ] {
            let rows = art(shape);
            assert!(!rows.is_empty(), "{shape:?} 没有位图");
            let width = rows[0].len();
            assert!(width > 0, "{shape:?} 宽度为 0");
            assert!(
                rows.iter().all(|row| row.len() == width),
                "{shape:?} 各行宽度不一致"
            );
            assert!(
                rows.iter().any(|row| row.contains('#')),
                "{shape:?} 没有像素"
            );
        }
    }

    #[test]
    fn cursor_rect_keeps_cursor_on_screen() {
        let cursor = Cursor {
            x: 1279,
            y: 799,
            shape: Shape::Arrow,
        };
        let (x, y, width, height) = cursor_rect(cursor, 1280, 800, 1);
        assert_eq!(x + width, 1280);
        assert_eq!(y + height, 800);
    }

    #[test]
    fn cursor_rect_centers_large_shapes_on_the_hotspot() {
        let cursor = Cursor {
            x: 100,
            y: 100,
            shape: Shape::Move,
        };
        let (x, y, width, _height) = cursor_rect(cursor, 1280, 800, 1);
        assert_eq!(x + (width - 1) / 2, 100);
        assert!(y < 100);
    }

    #[test]
    fn shape_changes_are_reported_once() {
        let mut pointer = Pointer::new();
        assert!(pointer.set_shape(Shape::Hand));
        assert!(!pointer.set_shape(Shape::Hand));
        assert!(pointer.set_shape(Shape::Arrow));
        assert_eq!(pointer.cursor(), None);
        pointer.set_position((10, 20));
        assert_eq!(
            pointer.cursor(),
            Some(Cursor {
                x: 10,
                y: 20,
                shape: Shape::Arrow,
            })
        );
    }
}
