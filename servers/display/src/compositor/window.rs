//! 窗口生命周期：挂载、表面替换、标题、提交、销毁与焦点。

use hk_proto::display::{
    DISPLAY_RESIZE, SURFACE_FRAME_DONE, TITLE_MAX, WINDOW_CREATED, pack_window_size,
    unpack_window_size,
};
use hk_server_api::{
    capability_phys, close_handle, format::Line, map_attach, region_unmap, send, write,
};

use super::Compositor;
use super::surface::set_scanout;
use crate::actions::close_window;
use crate::compositor::render::{
    compose_region, damage_taskbar, damage_title, damage_window, surface_pixel,
};
use crate::surface::{Surface, clip};
use crate::theme;
use crate::window::{Window, raise, taskbar_height, title_height};

impl Compositor {
    /// 把表面绑定到窗口：分配窗口槽、放置几何并提升焦点。
    pub(super) fn attach_window(&mut self, surface_id: u64, sender: u64) {
        let window = self.create_window(surface_id);
        let _ = send(sender, WINDOW_CREATED, window.unwrap_or(0));
    }

    fn create_window(&mut self, surface_id: u64) -> Option<u64> {
        let slot = self.surfaces.iter().position(|entry| {
            entry
                .as_ref()
                .is_some_and(|surface| surface.id == surface_id)
        })?;
        let surface = self.surfaces[slot].take()?;
        let Some(window_slot) = self.windows.iter().position(Option::is_none) else {
            self.surfaces[slot] = Some(surface);
            write(b"hk-server-display: create full\n");
            return None;
        };
        let id = self.next_window_id;
        let width = surface.width;
        let height = surface.height;
        let offset = (id as usize - 1) * 16;
        let x = self
            .screen
            .width
            .saturating_sub(width)
            .saturating_div(2)
            .saturating_add(offset)
            .min(self.screen.width.saturating_sub(width));
        let desktop_height = self.screen.height.saturating_sub(taskbar_height());
        let y = desktop_height
            .saturating_sub(height + title_height())
            .saturating_div(2)
            .saturating_add(offset)
            .min(desktop_height.saturating_sub(height + title_height()));
        let previous = self.focus;
        self.windows[window_slot] = Some(Window {
            id,
            slot: surface.slot,
            surface,
            x,
            y,
            width,
            height,
            title: [0; TITLE_MAX],
            title_length: 0,
            restore: None,
            minimized: false,
        });
        self.next_window_id += 1;
        self.focus = id;
        raise(&mut self.windows, id);
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
            previous,
        );
        damage_taskbar(
            &self.screen,
            &self.windows,
            self.focus,
            self.font,
            self.seat.pointer.cursor(),
        );
        Some(id)
    }

    /// 用客户端转移的新表面替换窗口缓冲，尺寸以客户端声明为准。
    pub(super) fn replace_window_surface(&mut self, word: u64, handle: u64, sender: u64) {
        let (id, width, height) = unpack_window_size(word);
        let width = width as usize;
        let height = height as usize;
        let Some(index) = self
            .windows
            .iter()
            .position(|window| window.as_ref().is_some_and(|window| window.id == id))
        else {
            if handle != 0 && handle != u64::MAX {
                let _ = close_handle(handle);
            }
            return;
        };
        if handle == 0 || handle == u64::MAX || width == 0 || height == 0 {
            if handle != 0 && handle != u64::MAX {
                let _ = close_handle(handle);
            }
            return;
        }
        let bytes = width as u64 * height as u64 * 4;
        let Some((_, frame_length)) = capability_phys(handle) else {
            let _ = close_handle(handle);
            write(b"hk-server-display: resize no-frame\n");
            return;
        };
        if frame_length < bytes {
            let _ = close_handle(handle);
            write(b"hk-server-display: resize short-frame\n");
            return;
        }
        let Some(window) = self.windows[index] else {
            let _ = close_handle(handle);
            return;
        };
        if window.surface.client != sender {
            let _ = close_handle(handle);
            write(b"hk-server-display: resize owner mismatch\n");
            return;
        }
        let (surface_id, client, slot) = (
            window.surface.id,
            window.surface.client,
            window.surface.slot,
        );
        let Some((region, virt)) = map_attach(handle) else {
            let _ = close_handle(handle);
            write(b"hk-server-display: resize map-failed\n");
            return;
        };
        if capability_phys(handle).is_none_or(|(_, length)| length < bytes) {
            let _ = region_unmap(region);
            let _ = close_handle(handle);
            write(b"hk-server-display: resize map-failed\n");
            return;
        }
        set_scanout(handle, surface_id, true);
        let old = window.surface;
        let mut replacement = Surface::new(
            surface_id, client, slot, width, height, virt, region, handle,
        );
        replacement.cursor = old.cursor;
        if let Some(window) = self.windows[index].as_mut() {
            window.surface = replacement;
        }
        set_scanout(old.handle, old.id, false);
        let _ = region_unmap(old.region);
        let _ = close_handle(old.handle);
        if let Some(window) = self.windows[index] {
            damage_window(
                &self.screen,
                &self.windows,
                self.focus,
                self.font,
                self.seat.pointer.cursor(),
                &window,
            );
        }
        Line::new()
            .text(b"hk-server-display: surface resize ")
            .number(id)
            .text(b" ")
            .number(width as u64)
            .text(b"x")
            .number(height as u64)
            .text(b"\n")
            .finish();
    }

    /// 通知客户端窗口内容尺寸；提交帧后若仍不一致会再次补发。
    pub(super) fn notify_resize(&self, id: u64) {
        if let Some(window) = self.windows.iter().flatten().find(|window| window.id == id) {
            let _ = send(
                window.surface.client,
                DISPLAY_RESIZE,
                pack_window_size(id, window.width as u32, window.height as u32),
            );
        }
    }

    /// 应用窗口标题；只在内容变化时重绘，自检模式校验像素。
    pub(super) fn set_window_title(&mut self, id: u64, text: &[u8]) {
        let mut changed = false;
        if let Some(window) = self
            .windows
            .iter_mut()
            .flatten()
            .find(|window| window.id == id)
        {
            let mut count = text.len().min(TITLE_MAX);
            while count > 0 && core::str::from_utf8(&text[..count]).is_err() {
                count -= 1;
            }
            if window.title_length != count || window.title[..count] != text[..count] {
                window.title[..count].copy_from_slice(&text[..count]);
                window.title_length = count;
                changed = true;
            }
        }
        if !changed {
            return;
        }
        damage_title(
            &self.screen,
            &self.windows,
            self.focus,
            self.font,
            self.seat.pointer.cursor(),
            id,
        );
        damage_taskbar(
            &self.screen,
            &self.windows,
            self.focus,
            self.font,
            self.seat.pointer.cursor(),
        );
        if !self.display_test {
            return;
        }
        let titled = self
            .windows
            .iter()
            .flatten()
            .find(|window| window.id == id)
            .is_some_and(|window| {
                let palette = theme::theme();
                let color = if window.id == self.focus {
                    palette.title_focus
                } else {
                    palette.title_color
                };
                (window.y + 4..(window.y + title_height()).min(self.screen.height)).any(|row| {
                    (window.x + 4..(window.x + window.width).min(self.screen.width))
                        .any(|column| self.screen.get(column, row) != color)
                })
            });
        if titled {
            write(b"hk-server-display: title ok\n");
        }
    }

    /// 提交表面 damage 并等待客户端下一帧。
    pub(super) fn commit_surface(&mut self, surface_id: u64, sender: u64) {
        let slot = self.windows.iter().position(|window| {
            window
                .as_ref()
                .is_some_and(|window| window.surface.id == surface_id)
        });
        let Some(slot) = slot else {
            if let Some(surface) = self
                .surfaces
                .iter_mut()
                .flatten()
                .find(|surface| surface.id == surface_id)
            {
                let _ = surface.take_damage();
            }
            let _ = send(sender, SURFACE_FRAME_DONE, surface_id);
            return;
        };
        let damage = self.windows[slot]
            .as_mut()
            .and_then(|window| window.surface.take_damage());
        let Some(window) = self.windows[slot] else {
            return;
        };
        match damage {
            Some(damage) => {
                let body = clip(
                    damage.x,
                    damage.y,
                    damage.width,
                    damage.height,
                    window.width,
                    window.height,
                );
                if let Some(body) = body {
                    compose_region(
                        &self.screen,
                        &self.windows,
                        self.focus,
                        self.font,
                        self.seat.pointer.cursor(),
                        window.x + body.x,
                        window.y + title_height() + body.y,
                        body.width,
                        body.height,
                    );
                }
                Line::new()
                    .text(b"hk-server-display: surface ")
                    .number(surface_id)
                    .text(b" ok\n")
                    .finish();
            }
            None => {
                damage_window(
                    &self.screen,
                    &self.windows,
                    self.focus,
                    self.font,
                    self.seat.pointer.cursor(),
                    &window,
                );
                let expected = surface_pixel(&window, 0, 0);
                let actual = self.screen.get(window.x, window.y + title_height());
                if expected != actual {
                    write(b"hk-server-display: composite mismatch\n");
                } else {
                    Line::new()
                        .text(b"hk-server-display: composite ok ")
                        .number(window.id)
                        .text(b" ")
                        .number(window.width as u64)
                        .text(b"x")
                        .number(window.height as u64)
                        .text(b"\n")
                        .finish();
                }
            }
        }
        let _ = send(sender, SURFACE_FRAME_DONE, surface_id);
        if let Some(window) = self.windows[slot].as_ref()
            && (window.width != window.surface.width || window.height != window.surface.height)
        {
            let _ = send(
                window.surface.client,
                DISPLAY_RESIZE,
                pack_window_size(window.id, window.width as u32, window.height as u32),
            );
        }
    }

    /// 销毁表面及其窗口。
    pub(super) fn destroy_surface(&mut self, surface_id: u64) {
        self.discard_surface(surface_id);
        if let Some(window) = self
            .windows
            .iter()
            .flatten()
            .find(|window| window.surface.id == surface_id)
            .copied()
        {
            close_window(
                &self.screen,
                &mut self.windows,
                &mut self.focus,
                self.font,
                self.seat.pointer.cursor(),
                window.id,
            );
        }
    }

    /// 把焦点切到指定窗口并重绘受影响区域。
    pub(super) fn focus_window(&mut self, id: u64) {
        if !self.windows.iter().flatten().any(|window| window.id == id) {
            return;
        }
        let previous = self.focus;
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
            previous,
        );
        damage_taskbar(
            &self.screen,
            &self.windows,
            self.focus,
            self.font,
            self.seat.pointer.cursor(),
        );
        Line::new()
            .text(b"hk-server-display: focus ")
            .number(id)
            .text(b"\n")
            .finish();
    }
}
