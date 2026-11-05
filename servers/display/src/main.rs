//! display：把帧缓冲映射到用户态并绘制测试图案。
//!
//! 只有本服务持有帧缓冲访问授权；绘制后回读像素确认映射确实指向显示内存。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;
#[cfg(target_os = "none")]
use hk_server_api::framebuffer;
#[cfg(target_os = "none")]
use hk_server_api::{exit, write, write_u64};

/// 测试图案的八种颜色。
#[cfg(target_os = "none")]
const BARS: [u32; 8] = [
    0x00FF_0000,
    0x0000_FF00,
    0x0000_00FF,
    0x00FF_FF00,
    0x00FF_00FF,
    0x0000_FFFF,
    0x00FF_FFFF,
    0x0080_8080,
];

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(_endpoint: u64) -> ! {
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

    let mapped = framebuffer::map(framebuffer::MAP_ADDRESS);
    if mapped == 0 {
        write(b"hk-server-display: map failed\n");
        exit(2)
    }

    let pixels = framebuffer::MAP_ADDRESS as *mut u32;
    let stride_pixels = framebuffer.stride as usize / 4;
    let bars = BARS.len() as u64;
    for y in 0..framebuffer.height as usize {
        for x in 0..framebuffer.width as usize {
            let index = (x as u64 * bars / u64::from(framebuffer.width)) as usize;
            // SAFETY: 帧缓冲已按描述符长度映射到该虚拟地址。
            unsafe {
                pixels
                    .add(y * stride_pixels + x)
                    .write_volatile(BARS[index]);
            }
        }
    }

    let last_index =
        (framebuffer.height as usize - 1) * stride_pixels + framebuffer.width as usize - 1;
    // SAFETY: 首尾像素都在已映射范围内。
    let (first, last) = unsafe {
        (
            pixels.read_volatile(),
            pixels.add(last_index).read_volatile(),
        )
    };
    if first != BARS[0] || last != BARS[7] {
        write(b"hk-server-display: verify failed\n");
        exit(3)
    }

    write(b"hk-server-display: ok ");
    write_u64(u64::from(framebuffer.width));
    write(b"x");
    write_u64(u64::from(framebuffer.height));
    write(b" stride=");
    write_u64(u64::from(framebuffer.stride));
    write(b" mapped=");
    write_u64(mapped);
    write(b"\n");
    exit(0)
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(not(target_os = "none"))]
fn main() {
    eprintln!("hk-server-display 只在 x86_64-unknown-none 目标上构建");
}
