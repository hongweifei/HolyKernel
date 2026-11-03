//! HolyKernel x86_64 QEMU 启动入口。
//!
//! Limine 载入内核后跳转到 `_start`：初始化 COM1、读取 Limine 响应、构造并校验
//! `BootInfo`，然后按 QEMU `isa-debug-exit` 协议退出。
//!
//! 本 crate 只在 `x86_64-unknown-none` 上构建；宿主构建保留占位入口，便于
//! `cargo clippy --workspace --all-targets` 与工作区测试。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::format;
#[cfg(target_os = "none")]
use alloc::string::String;
#[cfg(target_os = "none")]
use core::alloc::{GlobalAlloc, Layout};
#[cfg(target_os = "none")]
use core::cell::UnsafeCell;
#[cfg(target_os = "none")]
use core::fmt::Write;
#[cfg(target_os = "none")]
use core::panic::PanicInfo;
#[cfg(target_os = "none")]
use core::sync::atomic::{AtomicUsize, Ordering};

#[cfg(target_os = "none")]
use hk_hal::{BootInfo, Console, PhysAddr, VirtAddr};
#[cfg(target_os = "none")]
use hk_platform_x86_64::{COM1, FrameAllocator, PageFlags, PageTable, Serial};
#[cfg(target_os = "none")]
use hk_test::ExitCode;

/// 内核堆大小。
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 1024 * 1024;

#[cfg(target_os = "none")]
#[repr(align(4096))]
struct Heap(UnsafeCell<[u8; HEAP_SIZE]>);

// SAFETY: 堆内存只由 `BumpAllocator` 通过原子游标分配，返回的区间互不重叠。
#[cfg(target_os = "none")]
unsafe impl Sync for Heap {}

#[cfg(target_os = "none")]
static HEAP: Heap = Heap(UnsafeCell::new([0; HEAP_SIZE]));

#[cfg(target_os = "none")]
struct BumpAllocator {
    next: AtomicUsize,
}

// SAFETY: `alloc` 从静态堆中切出不重叠区间；`dealloc` 不回收，符合启动期
// 分配器的语义。
#[cfg(target_os = "none")]
unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let base = HEAP.0.get() as usize;
        let mut current = self.next.load(Ordering::Relaxed);
        loop {
            let start = (base + current + layout.align() - 1) & !(layout.align() - 1);
            let end = start + layout.size();
            if end > base + HEAP_SIZE {
                return core::ptr::null_mut();
            }
            let offset = end - base;
            match self.next.compare_exchange_weak(
                current,
                offset,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return start as *mut u8,
                Err(actual) => current = actual,
            }
        }
    }

    unsafe fn dealloc(&self, _pointer: *mut u8, _layout: Layout) {}
}

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator {
    next: AtomicUsize::new(0),
};

#[cfg(target_os = "none")]
core::arch::global_asm!(
    ".section .text.entry",
    ".global _start",
    "_start:",
    "and rsp, -16",
    "xor rbp, rbp",
    "call kmain",
    "ud2",
);

#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
extern "C" fn kmain() -> ! {
    let console = Serial::new(COM1);
    console.init();
    console.write_str("HolyKernel x86_64\n");
    hk_platform_x86_64::cpu::init();

    let info = match hk_boot::boot_info() {
        Ok(info) => info,
        Err(error) => {
            announce(&console, None, &format!("启动失败：{error}\n"));
            hk_test::exit(ExitCode::Failure(1));
        }
    };

    let screen = framebuffer_console(&info);
    if let Some(screen) = &screen {
        screen.clear();
        announce(
            &console,
            None,
            &format!(
                "帧缓冲控制台: {}x{} 字符\n",
                screen.columns(),
                screen.rows()
            ),
        );
    }

    announce(&console, screen.as_ref(), "HolyKernel x86_64\n");
    announce(&console, screen.as_ref(), &describe(&info));

    if let Err(error) = info.validate() {
        announce(
            &console,
            screen.as_ref(),
            &format!("启动信息校验失败：{error}\n"),
        );
        hk_test::exit(ExitCode::Failure(2));
    }

    announce(&console, screen.as_ref(), "启动信息校验通过\n");

    if let Err(error) = init_interrupts(&info) {
        announce(
            &console,
            screen.as_ref(),
            &format!("中断初始化失败：{error}\n"),
        );
        hk_test::exit(ExitCode::Failure(3));
    }
    announce(&console, screen.as_ref(), "本地 APIC 就绪\n");

    match hk_boot::cmdline() {
        Some(line) if line.contains("page-fault-test") => {
            announce(&console, screen.as_ref(), "触发异常测试：空指针写入\n");
            // SAFETY: 故意写入空指针触发 #PF，用于验证异常现场输出。
            unsafe {
                core::ptr::write_volatile(core::ptr::null_mut::<u64>(), 1);
            }
        }
        Some(line) if line.contains("fault-test") => {
            announce(&console, screen.as_ref(), "触发异常测试：ud2\n");
            // SAFETY: 故意执行非法指令触发 #UD，用于验证异常现场输出。
            unsafe {
                core::arch::asm!("ud2", options(noreturn));
            }
        }
        Some(line) if line.contains("timer-test") => {
            run_timer_test(&console);
        }
        Some(line) if line.contains("fb-test") => {
            announce(
                &console,
                screen.as_ref(),
                "帧缓冲测试：画面保持 2 秒后退出\n",
            );
            hk_platform_x86_64::cpu::arm_timer(200);
            while !hk_platform_x86_64::cpu::timer_finished() {
                // SAFETY: `hlt` 等到下一次定时器中断。
                unsafe {
                    core::arch::asm!("hlt", options(nomem, nostack));
                }
            }
            hk_platform_x86_64::cpu::disarm_timer();
        }
        _ => {}
    }

    hk_test::exit(ExitCode::Success);
}

#[cfg(target_os = "none")]
fn framebuffer_console(info: &BootInfo) -> Option<hk_platform_x86_64::FramebufferConsole> {
    let framebuffer = info.framebuffer?;
    let direct_map = info.direct_map?;
    Some(hk_platform_x86_64::FramebufferConsole::new(
        &framebuffer,
        direct_map.virt_base.get(),
    ))
}

#[cfg(target_os = "none")]
fn announce(console: &Serial, screen: Option<&hk_platform_x86_64::FramebufferConsole>, text: &str) {
    console.write_str(text);
    if let Some(screen) = screen {
        screen.write_str(text);
    }
}

#[cfg(target_os = "none")]
fn init_interrupts(info: &BootInfo) -> Result<(), String> {
    let direct_map = info
        .direct_map
        .ok_or_else(|| String::from("启动信息缺少 direct map"))?;
    let direct_map_base = direct_map.virt_base.get();
    let mut allocator = FrameAllocator::new(info.memory_map, info.kernel_image)
        .ok_or_else(|| String::from("没有可用的物理内存区域"))?;

    let paging = PageTable::new(direct_map_base);
    let lapic_phys = PhysAddr::new(0xFEE0_0000);
    let lapic_virt = VirtAddr::new(direct_map_base + lapic_phys.get());
    paging
        .map(lapic_virt, lapic_phys, PageFlags::MMIO, &mut allocator)
        .map_err(|error| format!("映射 LAPIC 失败：{error:?}"))?;

    hk_platform_x86_64::cpu::init_apic(lapic_virt.get());
    Ok(())
}

#[cfg(target_os = "none")]
fn run_timer_test(console: &Serial) -> ! {
    console.write_str("启动本地 APIC 定时器，等待 5 次中断\n");

    hk_platform_x86_64::cpu::arm_timer(5);

    while !hk_platform_x86_64::cpu::timer_finished() {
        // SAFETY: `hlt` 等到下一次中断；中断由本地 APIC 定时器投递。
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }

    hk_platform_x86_64::cpu::disarm_timer();
    let ticks = hk_platform_x86_64::cpu::timer_ticks();
    let mut report = String::new();
    let _ = writeln!(report, "定时器中断 {ticks} 次");
    console.write_str(&report);

    hk_test::exit(ExitCode::Success)
}

#[cfg(target_os = "none")]
fn describe(info: &BootInfo) -> String {
    let mut report = String::new();
    let _ = writeln!(report, "  架构: {}", info.arch);
    let _ = writeln!(report, "  内存 map:");
    for region in info.memory_map {
        let _ = writeln!(
            report,
            "    {:#012x} + {:#010x}  {}",
            region.base.get(),
            region.len,
            region.kind.name()
        );
    }
    let _ = writeln!(
        report,
        "  内核镜像: {:#012x} + {:#010x}",
        info.kernel_image.base.get(),
        info.kernel_image.len
    );
    match info.framebuffer {
        Some(framebuffer) => {
            let _ = writeln!(
                report,
                "  帧缓冲: {:#012x} {}x{} stride={}",
                framebuffer.phys.get(),
                framebuffer.width,
                framebuffer.height,
                framebuffer.stride
            );
        }
        None => {
            let _ = writeln!(report, "  帧缓冲: 无");
        }
    }
    match info.direct_map {
        Some(direct_map) => {
            let _ = writeln!(
                report,
                "  direct map: {:#012x} -> {:#018x} + {:#010x}",
                direct_map.phys_base.get(),
                direct_map.virt_base.get(),
                direct_map.len
            );
        }
        None => {
            let _ = writeln!(report, "  direct map: 无");
        }
    }
    match info.rsdp {
        Some(rsdp) => {
            let _ = writeln!(report, "  RSDP: {:#012x}", rsdp.get());
        }
        None => {
            let _ = writeln!(report, "  RSDP: 无");
        }
    }
    let _ = writeln!(report, "  服务器模块: {}", info.server_modules.len());
    report
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    let console = Serial::new(COM1);
    let mut report = String::new();
    let _ = writeln!(report, "\n内核 panic：{info}");
    console.write_str(&report);
    hk_test::exit(ExitCode::Failure(70))
}

#[cfg(not(target_os = "none"))]
fn main() {
    eprintln!("hk-x86_64 只在 x86_64-unknown-none 目标上构建");
}
