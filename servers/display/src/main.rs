//! display：帧缓冲合成器与窗口服务。
//!
//! 只有本服务持有帧缓冲授权。客户端把共享 Frame 作为窗口表面，经句柄转移
//! 交给本服务；合成按区域重绘，避免每次提交都重画整屏。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(any(target_os = "none", test))]
mod pointer;
#[cfg(any(target_os = "none", test))]
mod resize;
#[cfg(any(target_os = "none", test))]
mod seat;
#[cfg(any(target_os = "none", test))]
mod surface;

#[cfg(target_os = "none")]
mod actions;
#[cfg(target_os = "none")]
mod compositor;
#[cfg(target_os = "none")]
mod theme;
#[cfg(target_os = "none")]
mod window;

#[cfg(target_os = "none")]
use compositor::{Compositor, draw_desktop};
#[cfg(target_os = "none")]
use hk_server_api::{CapabilityKind, MemoryClient, exit, format::Line, framebuffer, write};
#[cfg(target_os = "none")]
use window::Screen;

/// 服务入口：建立帧缓冲与后备缓冲，随后交给合成器主循环。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let Some(framebuffer) = framebuffer::query() else {
        write(b"hk-server-display: no framebuffer\n");
        exit(1)
    };
    if framebuffer.width == 0
        || framebuffer.height == 0
        || framebuffer.stride < framebuffer.width * 4
    {
        write(b"hk-server-display: invalid framebuffer\n");
        exit(1)
    }

    let Some((_mapped, framebuffer_virt)) = framebuffer::map() else {
        write(b"hk-server-display: map failed\n");
        exit(2)
    };

    let width = framebuffer.width as usize;
    let height = framebuffer.height as usize;
    let bytes = width * height * 4;
    let Some((_back_handle, _back_region, back_virt)) =
        MemoryClient::new(endpoint).map(CapabilityKind::Surface, bytes)
    else {
        write(b"hk-server-display: back buffer failed\n");
        exit(2)
    };
    let screen = Screen::new(
        back_virt as *mut u32,
        framebuffer_virt as *mut u32,
        width,
        height,
        framebuffer.stride as usize / 4,
    );

    let mut theme_buffer = [0_u8; 128];
    let theme_name = theme::select(hk_server_api::boot_cmdline(&mut theme_buffer));
    let mut test_buffer = [0_u8; 128];
    let display_test = hk_server_api::boot_cmdline(&mut test_buffer)
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
        .number(u64::from(framebuffer.width))
        .text(b"x")
        .number(u64::from(framebuffer.height))
        .text(b"\n")
        .finish();

    Compositor::new(endpoint, screen, font, display_test).run()
}

hk_server_api::server_boilerplate!();
