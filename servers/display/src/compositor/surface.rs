//! 表面生命周期：创建、扫描输出驻留、damage、输入区域与销毁。

use hk_proto::display::{SURFACE_CREATED, unpack_size, unpack_surface_rect};
use hk_proto::pointer::unpack_surface_cursor;
use hk_server_api::{
    capability_phys, capability_scanout, close_handle, format::Line, map_attach, region_unmap,
    send, write,
};

use super::Compositor;
use crate::pointer;
use crate::resize::Rect;
use crate::surface::{MAX_SURFACES, Surface};

impl Compositor {
    /// 为客户端创建表面并把编号回送；失败返回 0。
    pub(super) fn create_surface(&mut self, word: u64, sender: u64, handle: u64) {
        let (width, height) = unpack_size(word);
        let surface = self.register_surface(sender, width as usize, height as usize, handle);
        let _ = send(sender, SURFACE_CREATED, surface.unwrap_or(0));
    }

    fn register_surface(
        &mut self,
        client: u64,
        width: usize,
        height: usize,
        handle: u64,
    ) -> Option<u64> {
        if handle == 0 || width == 0 || height == 0 {
            if handle != 0 {
                let _ = close_handle(handle);
            }
            write(b"hk-server-display: create invalid\n");
            return None;
        }
        let mut slot = None;
        for index in 0..MAX_SURFACES {
            if self.surfaces[index].is_none()
                && !self
                    .windows
                    .iter()
                    .flatten()
                    .any(|window| window.slot == index)
            {
                slot = Some(index);
                break;
            }
        }
        let Some(slot) = slot else {
            let _ = close_handle(handle);
            write(b"hk-server-display: create full\n");
            return None;
        };
        let bytes = width as u64 * height as u64 * 4;
        let Some((_, frame_length)) = capability_phys(handle) else {
            let _ = close_handle(handle);
            write(b"hk-server-display: create no-frame\n");
            return None;
        };
        if frame_length < bytes {
            let _ = close_handle(handle);
            write(b"hk-server-display: create short-frame\n");
            return None;
        }
        let id = self.next_surface_id;
        let Some((region, virt)) = map_attach(handle) else {
            let _ = close_handle(handle);
            write(b"hk-server-display: create map-failed\n");
            return None;
        };
        if capability_phys(handle).is_none_or(|(_, length)| length < bytes) {
            let _ = region_unmap(region);
            let _ = close_handle(handle);
            write(b"hk-server-display: create map-failed\n");
            return None;
        }
        set_scanout(handle, id, true);
        self.surfaces[slot] = Some(Surface::new(
            id, client, slot, width, height, virt, region, handle,
        ));
        self.next_surface_id += 1;
        Some(id)
    }

    /// 在未绑定表面或已绑定窗口上记录 damage。
    pub(super) fn mark_surface_damage(&mut self, payload: &[u8], length: usize) {
        if let Some((surface, x, y, width, height)) =
            unpack_surface_rect(&payload[..length.min(payload.len())])
        {
            self.mark_damage(
                surface,
                x as usize,
                y as usize,
                width as usize,
                height as usize,
            );
        }
    }

    fn mark_damage(&mut self, surface_id: u64, x: usize, y: usize, width: usize, height: usize) {
        if let Some(window) = self
            .windows
            .iter_mut()
            .flatten()
            .find(|window| window.surface.id == surface_id)
        {
            window.surface.mark_damage(x, y, width, height);
            return;
        }
        if let Some(surface) = self
            .surfaces
            .iter_mut()
            .flatten()
            .find(|surface| surface.id == surface_id)
        {
            surface.mark_damage(x, y, width, height);
        }
    }

    /// 设置未绑定表面或已绑定窗口的输入区域。
    pub(super) fn set_surface_input_region(&mut self, payload: &[u8], length: usize) {
        let Some((surface, x, y, width, height)) =
            unpack_surface_rect(&payload[..length.min(payload.len())])
        else {
            return;
        };
        let region = Rect {
            x: x as usize,
            y: y as usize,
            width: width as usize,
            height: height as usize,
        };
        if let Some(window) = self
            .windows
            .iter_mut()
            .flatten()
            .find(|window| window.surface.id == surface)
        {
            window.surface.set_input_region(region);
            return;
        }
        if let Some(entry) = self
            .surfaces
            .iter_mut()
            .flatten()
            .find(|entry| entry.id == surface)
        {
            entry.set_input_region(region);
        }
    }

    /// 应用窗口正文的光标形状，并在指针已落在该窗口时刷新悬停。
    pub(super) fn set_surface_cursor(&mut self, word: u64) {
        let (surface, shape) = unpack_surface_cursor(word);
        let Some(shape) = shape else {
            return;
        };
        let found =
            pointer::set_surface_cursor(&mut self.windows, &mut self.surfaces, surface, shape);
        Line::new()
            .text(b"hk-server-display: cursor hint ")
            .number(surface)
            .text(b" ")
            .text(pointer::label(shape).as_bytes())
            .text(if found {
                b"\n".as_slice()
            } else {
                b" missed\n".as_slice()
            })
            .finish();
        if found && let Some((x, y)) = self.seat.pointer.position() {
            let hover = pointer::hover_at(&self.windows, &self.screen, x, y);
            let shape = pointer::shape_for_hover(hover, &self.windows);
            self.apply_hover(hover, shape);
            self.redraw_cursor();
        }
    }

    /// 丢弃未绑定表面的缓冲。
    pub(super) fn discard_surface(&mut self, surface_id: u64) {
        let slot = self.surfaces.iter().position(|entry| {
            entry
                .as_ref()
                .is_some_and(|surface| surface.id == surface_id)
        });
        if let Some(slot) = slot
            && let Some(surface) = self.surfaces[slot].take()
        {
            set_scanout(surface.handle, surface.id, false);
            let _ = region_unmap(surface.region);
            let _ = close_handle(surface.handle);
        }
    }
}

/// 设置表面 scanout 驻留并记录状态切换。
pub(crate) fn set_scanout(handle: u64, surface_id: u64, enabled: bool) {
    let _ = capability_scanout(handle, enabled);
    Line::new()
        .text(b"hk-server-display: scanout ")
        .number(surface_id)
        .text(if enabled { b" on\n" } else { b" off\n" })
        .finish();
}
