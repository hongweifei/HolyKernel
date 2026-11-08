//! 输入处理：指针移动、按钮、键盘与滚轮，以及光标与悬停绘制。

use hk_proto::display::{DISPLAY_CLICK, DISPLAY_CLICK_AT, pack_click_at};
use hk_proto::input::{unpack_pointer, unpack_wheel};
use hk_proto::pointer::Shape;
use hk_server_api::{format::Line, send, tsc, tsc_frequency, write, write_u64};

use super::Compositor;
use super::surface::set_scanout;
use crate::actions::{
    close_window, cycle_focus, minimize_window, route_key, route_wheel, toggle_maximize,
};
use crate::compositor::render::{
    compose_region, damage_taskbar, damage_title, damage_window, move_window, resize_window,
};
use crate::pointer::{self, Hover};
use crate::resize::{self, Rect};
use crate::window::{
    Drag, MIN_WINDOW_HEIGHT, MIN_WINDOW_WIDTH, Resize, close_at, maximize_at, minimize_at, raise,
    resize_at, taskbar_at, taskbar_height, taskbar_top, title_height, window_at,
};

/// Tab 键字节：切换焦点。
const TAB: u8 = b'\t';

impl Compositor {
    /// 应用悬停目标：更新光标形状与按钮高亮。
    pub(super) fn apply_hover(&mut self, hover: Hover, shape: Shape) {
        if self.seat.pointer.set_shape(shape) {
            write(b"hk-server-display: cursor ");
            write(pointer::label(shape).as_bytes());
            write(b"\n");
        }
        let previous = self.screen.hover();
        if previous == Some(hover) {
            return;
        }
        self.screen.set_hover(Some(hover));
        for target in [previous, Some(hover)] {
            if let Some((x, y, width, height)) =
                pointer::hover_rect(&self.windows, &self.screen, target)
            {
                compose_region(
                    &self.screen,
                    &self.windows,
                    self.focus,
                    self.font,
                    None,
                    x,
                    y,
                    width,
                    height,
                );
            }
        }
    }

    /// 擦除旧光标：重绘它覆盖的矩形但不画光标。
    fn erase_cursor(&mut self) {
        if let Some(cursor) = self.seat.pointer.cursor() {
            let (x, y, width, height) = pointer::cursor_rect(
                cursor,
                self.screen.width,
                self.screen.height,
                crate::theme::scale(),
            );
            compose_region(
                &self.screen,
                &self.windows,
                self.focus,
                self.font,
                None,
                x,
                y,
                width,
                height,
            );
        }
    }

    /// 在当前指针位置重绘光标。
    pub(super) fn redraw_cursor(&self) {
        if let Some(cursor) = self.seat.pointer.cursor() {
            let (x, y, width, height) = pointer::cursor_rect(
                cursor,
                self.screen.width,
                self.screen.height,
                crate::theme::scale(),
            );
            compose_region(
                &self.screen,
                &self.windows,
                self.focus,
                self.font,
                Some(cursor),
                x,
                y,
                width,
                height,
            );
        }
    }

    /// 处理指针移动：拖拽、缩放、悬停与光标形状。
    pub(super) fn input_pointer(&mut self, word: u64) {
        static FIRST_POINTER: core::sync::atomic::AtomicBool =
            core::sync::atomic::AtomicBool::new(true);
        if FIRST_POINTER.swap(false, core::sync::atomic::Ordering::Relaxed) {
            write(b"hk-server-display: pointer ready\n");
        }
        let mut drag = self.drag;
        let mut resize = self.resize;
        let (x, y) = unpack_pointer(word);
        let next = pointer::normalize(x, y, self.screen.width, self.screen.height);
        self.erase_cursor();
        if let Some(state) = resize
            && let Some(window) = self
                .windows
                .iter()
                .flatten()
                .find(|window| window.id == state.id)
                .copied()
        {
            let desktop_height = self.screen.height.saturating_sub(taskbar_height());
            let delta_x = next.0 as isize - state.start_x as isize;
            let delta_y = next.1 as isize - state.start_y as isize;
            let next_rect = resize::resized(
                state.original,
                state.edge,
                delta_x,
                delta_y,
                self.screen.width,
                desktop_height,
                title_height(),
                MIN_WINDOW_WIDTH,
                MIN_WINDOW_HEIGHT,
            );
            if (next_rect.x, next_rect.y, next_rect.width, next_rect.height)
                != (window.x, window.y, window.width, window.height)
            {
                resize_window(
                    &self.screen,
                    &mut self.windows,
                    self.focus,
                    self.font,
                    window,
                    next_rect.x,
                    next_rect.y,
                    next_rect.width,
                    next_rect.height,
                );
                if !state.resized {
                    resize = Some(Resize {
                        resized: true,
                        ..state
                    });
                }
            }
        } else if let Some(state) = drag
            && let Some(window) = self
                .windows
                .iter()
                .flatten()
                .find(|window| window.id == state.id)
                .copied()
        {
            let desktop_height = self.screen.height.saturating_sub(taskbar_height());
            let (new_x, new_y) = resize::dragged(
                next,
                (state.offset_x, state.offset_y),
                window.width,
                window.height,
                (self.screen.width, desktop_height),
                title_height(),
            );
            if (new_x, new_y) != (window.x, window.y) {
                move_window(
                    &self.screen,
                    &mut self.windows,
                    self.focus,
                    self.font,
                    window,
                    new_x,
                    new_y,
                );
                if !state.moved {
                    drag = Some(Drag {
                        moved: true,
                        ..state
                    });
                    Line::new()
                        .text(b"hk-server-display: drag ")
                        .number(state.id)
                        .text(b" ")
                        .number(new_x as u64)
                        .text(b",")
                        .number(new_y as u64)
                        .text(b"\n")
                        .finish();
                }
            }
        }
        self.seat.pointer.set_position(next);
        let hover = if drag.is_some() || resize.is_some() {
            Hover::None
        } else {
            pointer::hover_at(&self.windows, &self.screen, next.0, next.1)
        };
        let shape = if let Some(state) = resize {
            pointer::shape_for_edges(state.edge)
        } else if drag.is_some() {
            Shape::Move
        } else {
            pointer::shape_for_hover(hover, &self.windows)
        };
        self.apply_hover(hover, shape);
        self.redraw_cursor();
        self.seat
            .route_pointer(&self.windows, hover, next.0, next.1);
        self.drag = drag;
        self.resize = resize;
    }

    /// 处理鼠标按钮：窗口命中、任务栏、拖拽、缩放与关闭。
    pub(super) fn input_button(&mut self, word: u64) {
        let mut drag = None;
        let mut resize = self.resize;
        if word == 0 {
            self.seat.pointer.set_pressed(false);
            if let Some(state) = resize
                && state.resized
            {
                if let Some(window) = self
                    .windows
                    .iter()
                    .flatten()
                    .find(|window| window.id == state.id)
                    .copied()
                {
                    Line::new()
                        .text(b"hk-server-display: resize ")
                        .number(window.id)
                        .text(b" ")
                        .number(window.width as u64)
                        .text(b"x")
                        .number(window.height as u64)
                        .text(b"\n")
                        .finish();
                }
                self.notify_resize(state.id);
            }
            if let Some((x, y)) = self.seat.pointer.position() {
                let hover = pointer::hover_at(&self.windows, &self.screen, x, y);
                let shape = pointer::shape_for_hover(hover, &self.windows);
                self.apply_hover(hover, shape);
            }
            self.redraw_cursor();
            self.drag = None;
            self.resize = None;
            return;
        }
        let was_pressed = self.seat.pointer.pressed();
        self.seat.pointer.set_pressed(true);
        resize = None;
        if let Some((x, y)) = self.seat.pointer.position() {
            let hover = pointer::hover_at(&self.windows, &self.screen, x, y);
            if !was_pressed {
                let clicks = self
                    .seat
                    .pointer
                    .register_press(hover, tsc(), tsc_frequency());
                if clicks == 2
                    && let Hover::Title(id) = hover
                {
                    toggle_maximize(
                        &self.screen,
                        &mut self.windows,
                        self.focus,
                        self.font,
                        self.seat.pointer.cursor(),
                        id,
                    );
                    write(b"hk-server-display: double click ");
                    write_u64(id);
                    write(b"\n");
                    let hover = pointer::hover_at(&self.windows, &self.screen, x, y);
                    let shape = pointer::shape_for_hover(hover, &self.windows);
                    self.apply_hover(hover, shape);
                    self.redraw_cursor();
                    self.drag = drag;
                    self.resize = resize;
                    return;
                }
            }
            if y >= taskbar_top(self.screen.height) {
                if let Some(id) =
                    taskbar_at(&self.windows, x, y, self.screen.width, self.screen.height)
                {
                    let old_focus = self.focus;
                    if let Some(window) = self
                        .windows
                        .iter_mut()
                        .flatten()
                        .find(|window| window.id == id)
                    {
                        window.minimized = false;
                        set_scanout(window.surface.handle, window.surface.id, true);
                    }
                    raise(&mut self.windows, id);
                    self.focus = id;
                    if let Some(window) = self
                        .windows
                        .iter()
                        .flatten()
                        .find(|window| window.id == id)
                        .copied()
                    {
                        damage_window(
                            &self.screen,
                            &self.windows,
                            self.focus,
                            self.font,
                            self.seat.pointer.cursor(),
                            &window,
                        );
                    }
                    damage_title(
                        &self.screen,
                        &self.windows,
                        self.focus,
                        self.font,
                        self.seat.pointer.cursor(),
                        old_focus,
                    );
                    damage_taskbar(
                        &self.screen,
                        &self.windows,
                        self.focus,
                        self.font,
                        self.seat.pointer.cursor(),
                    );
                    self.redraw_cursor();
                    Line::new()
                        .text(b"hk-server-display: taskbar ")
                        .number(id)
                        .text(b"\n")
                        .finish();
                }
                self.drag = drag;
                self.resize = resize;
                return;
            }
            let old_focus = self.focus;
            if let Some(window) = window_at(&self.windows, x, y) {
                if close_at(&window, x, y) {
                    close_window(
                        &self.screen,
                        &mut self.windows,
                        &mut self.focus,
                        self.font,
                        self.seat.pointer.cursor(),
                        window.id,
                    );
                    self.drag = drag;
                    self.resize = resize;
                    return;
                }
                if minimize_at(&window, x, y) {
                    minimize_window(
                        &self.screen,
                        &mut self.windows,
                        &mut self.focus,
                        self.font,
                        self.seat.pointer.cursor(),
                        window.id,
                    );
                    self.drag = drag;
                    self.resize = resize;
                    return;
                }
                if maximize_at(&window, x, y) {
                    toggle_maximize(
                        &self.screen,
                        &mut self.windows,
                        self.focus,
                        self.font,
                        self.seat.pointer.cursor(),
                        window.id,
                    );
                    self.drag = drag;
                    self.resize = resize;
                    return;
                }
                let id = window.id;
                raise(&mut self.windows, id);
                self.focus = id;
                if let Some(window) = self
                    .windows
                    .iter()
                    .flatten()
                    .find(|window| window.id == id)
                    .copied()
                {
                    damage_window(
                        &self.screen,
                        &self.windows,
                        self.focus,
                        self.font,
                        self.seat.pointer.cursor(),
                        &window,
                    );
                }
                damage_title(
                    &self.screen,
                    &self.windows,
                    self.focus,
                    self.font,
                    self.seat.pointer.cursor(),
                    old_focus,
                );
                damage_taskbar(
                    &self.screen,
                    &self.windows,
                    self.focus,
                    self.font,
                    self.seat.pointer.cursor(),
                );
                self.redraw_cursor();
                let on_title = y >= window.y && y < window.y + title_height();
                if let Some(edge) = resize_at(&window, x, y) {
                    resize = Some(Resize {
                        id,
                        edge,
                        start_x: x,
                        start_y: y,
                        original: Rect {
                            x: window.x,
                            y: window.y,
                            width: window.width,
                            height: window.height,
                        },
                        resized: false,
                    });
                } else if on_title {
                    drag = Some(Drag {
                        id,
                        offset_x: x - window.x,
                        offset_y: y - window.y,
                        moved: false,
                    });
                } else {
                    Line::new()
                        .text(b"hk-server-display: pointer click ")
                        .number(id)
                        .text(b"\n")
                        .finish();
                    if let Some(window) =
                        self.windows.iter().flatten().find(|window| window.id == id)
                    {
                        let _ = send(window.surface.client, DISPLAY_CLICK, id);
                        let body_x = x.saturating_sub(window.x) as u32;
                        let body_y = y.saturating_sub(window.y + title_height()) as u32;
                        let _ = send(
                            window.surface.client,
                            DISPLAY_CLICK_AT,
                            pack_click_at(id, body_x, body_y),
                        );
                    }
                }
            }
        }
        self.drag = drag;
        self.resize = resize;
    }

    /// 处理键盘：Tab 切换焦点，其余按键转发给焦点窗口。
    pub(super) fn input_key(&mut self, word: u64) {
        self.driver_input = true;
        self.console_key(word as u8);
    }

    /// 处理内核控制台字节。
    pub(super) fn console_key(&mut self, byte: u8) {
        if byte == TAB {
            cycle_focus(
                &self.screen,
                &mut self.windows,
                &mut self.focus,
                self.font,
                self.seat.pointer.cursor(),
            );
        } else {
            route_key(&self.windows, self.focus, byte);
        }
    }

    /// 处理滚轮：转发给焦点窗口。
    pub(super) fn input_wheel(&mut self, word: u64) {
        let delta = unpack_wheel(word);
        if delta != 0 {
            route_wheel(&self.windows, self.focus, delta);
        }
    }
}
