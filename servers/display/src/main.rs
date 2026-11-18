//! display：帧缓冲合成器与窗口服务。
//!
//! 只有本服务持有帧缓冲授权。客户端把共享 Frame 作为窗口表面，经句柄转移
//! 交给本服务；合成按区域重绘，避免每次提交都重画整屏。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(any(target_os = "none", test))]
mod pointer;
#[cfg(any(target_os = "none", test))]
mod resize;
#[cfg(any(target_os = "none", test))]
mod seat;
#[cfg(any(target_os = "none", test))]
mod surface;

// 服务堆大小：只用于接管期缓存客户端消息。
hk_user_api::server_heap!(16 * 1024);

#[cfg(target_os = "none")]
mod actions;
#[cfg(target_os = "none")]
mod compositor;
#[cfg(target_os = "none")]
mod scanout;
#[cfg(target_os = "none")]
mod selftest;
#[cfg(target_os = "none")]
mod theme;
#[cfg(target_os = "none")]
mod window;

#[cfg(target_os = "none")]
use alloc::vec::Vec;
#[cfg(target_os = "none")]
use compositor::{Compositor, draw_desktop};
#[cfg(target_os = "none")]
use hk_user_api::{
    CapabilityKind, MemoryClient, RIGHT_READ, RIGHT_WRITE, exit, format::Line, framebuffer,
    recv_data_handle, region_info, region_map, send, service_endpoint, write, yield_now,
};
#[cfg(target_os = "none")]
use window::Screen;

/// 服务入口：建立帧缓冲与后备缓冲，随后交给合成器主循环。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64, capacity: u64) -> ! {
    let mut command = [0_u8; 128];
    let command_line = hk_user_api::boot_cmdline(&mut command).unwrap_or("");
    let gpu_requested = command_line.contains("gpu.scanout");
    let mut pending: Vec<scanout::Pending> = Vec::new();
    let (width, height, stride, framebuffer_virt, scanout) = match framebuffer::query() {
        Some(framebuffer) if !gpu_requested => {
            if framebuffer.width == 0
                || framebuffer.height == 0
                || framebuffer.stride < framebuffer.width * 4
            {
                write(b"hk-server-display: invalid framebuffer\n");
                exit(1)
            }
            let Some((_mapped, virt)) = framebuffer::map() else {
                write(b"hk-server-display: map failed\n");
                exit(2)
            };
            (
                framebuffer.width as usize,
                framebuffer.height as usize,
                framebuffer.stride as usize / 4,
                virt,
                None,
            )
        }
        kernel => {
            let Some(scanout) = scanout::Scanout::adopt(endpoint, &mut pending) else {
                if kernel.is_none() {
                    write(b"hk-server-display: no framebuffer\n");
                } else {
                    write(b"hk-server-display: gpu scanout unavailable\n");
                }
                exit(1)
            };
            Line::new()
                .text(b"hk-server-display: scanout gpu ")
                .number(scanout.width as u64)
                .text(b"x")
                .number(scanout.height as u64)
                .text(b"\n")
                .finish();
            let fb = scanout.fb as u64;
            (
                scanout.width,
                scanout.height,
                scanout.stride,
                fb,
                Some(scanout),
            )
        }
    };

    let bytes = width * height * 4;
    let _ = capacity;
    let mut back = provider_surface(endpoint, bytes);
    if back.is_some() {
        write(b"hk-server-display: back provider ok\n");
    }
    if back.is_none() {
        back = MemoryClient::new(endpoint)
            .map(CapabilityKind::Surface, bytes)
            .map(|(_, _, virt)| virt);
    }
    let Some(back_virt) = back else {
        write(b"hk-server-display: back buffer failed\n");
        exit(2)
    };
    let screen = Screen::new(
        back_virt as *mut u32,
        framebuffer_virt as *mut u32,
        width,
        height,
        stride,
    );

    let mut theme_buffer = [0_u8; 128];
    let theme_name = theme::select(hk_user_api::boot_cmdline(&mut theme_buffer));
    let mut test_buffer = [0_u8; 128];
    let display_test = hk_user_api::boot_cmdline(&mut test_buffer)
        .is_some_and(|line| line.contains("display-test"));
    let font = hk_canvas::default_font_scaled(theme::scale());
    Line::new()
        .text(b"hk-server-display: font ")
        .text(if font.has_scalable() {
            b"scalable ".as_slice()
        } else {
            b"bitmap ".as_slice()
        })
        .number(u64::from(font.size()))
        .text(b"\n")
        .finish();
    Line::new()
        .text(b"hk-server-display: theme ")
        .text(theme_name.as_bytes())
        .text(b"\n")
        .finish();
    Line::new()
        .text(b"hk-server-display: scale ")
        .number(theme::scale() as u64)
        .text(b"\n")
        .finish();
    draw_desktop(&screen);
    Line::new()
        .text(b"hk-server-display: ready ")
        .number(width as u64)
        .text(b"x")
        .number(height as u64)
        .text(b"\n")
        .finish();

    if scanout.is_some() && command_line.contains("scanout-test") {
        if let Some(scanout) = &scanout {
            scanout.flush();
        }
        Line::new()
            .text(b"hk-server-display: scanout test ok\n")
            .finish();
        exit(0)
    }

    Compositor::new(endpoint, screen, font, display_test, scanout, pending).run()
}

/// 向 Surface Provider 申请懒加载后备表面；服务不可用时返回 `None`。
#[cfg(target_os = "none")]
fn provider_surface(endpoint: u64, bytes: usize) -> Option<u64> {
    let mut attempts = 0;
    while service_endpoint("hk-server-surface-provider") == 0 && attempts < 512 {
        attempts += 1;
        let _ = yield_now();
    }
    let provider = service_endpoint("hk-server-surface-provider");
    if provider == 0 || send(provider, hk_proto::display::SURFACE_ALLOC, bytes as u64) == u64::MAX {
        return None;
    }
    let mut empty = [0_u8; 0];
    let (label, size, _sender, _length, handle) = recv_data_handle(endpoint, &mut empty, provider);
    if label != hk_proto::display::SURFACE_ALLOCATED || handle == 0 || size as usize != bytes {
        return None;
    }
    let region = region_map(handle, 0, bytes, RIGHT_READ | RIGHT_WRITE, 0)?;
    let (virt, _) = region_info(region)?;
    Some(virt)
}

hk_user_api::server_boilerplate!();
