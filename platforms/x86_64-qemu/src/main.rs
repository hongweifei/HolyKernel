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
use hk_hal::{BootInfo, Console};
#[cfg(target_os = "none")]
use hk_platform_x86_64::{COM1, Serial};
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

    match hk_boot::boot_info() {
        Ok(info) => {
            console.write_str(&describe(&info));
            match info.validate() {
                Ok(()) => {
                    console.write_str("启动信息校验通过\n");
                    match hk_boot::cmdline() {
                        Some(line) if line.contains("page-fault-test") => {
                            console.write_str("触发异常测试：空指针写入\n");
                            // SAFETY: 故意写入空指针触发 #PF，用于验证异常现场输出。
                            unsafe {
                                core::ptr::write_volatile(core::ptr::null_mut::<u64>(), 1);
                            }
                        }
                        Some(line) if line.contains("fault-test") => {
                            console.write_str("触发异常测试：ud2\n");
                            // SAFETY: 故意执行非法指令触发 #UD，用于验证异常现场输出。
                            unsafe {
                                core::arch::asm!("ud2", options(noreturn));
                            }
                        }
                        _ => {}
                    }
                    hk_test::exit(ExitCode::Success);
                }
                Err(error) => {
                    let mut report = String::new();
                    let _ = writeln!(report, "启动信息校验失败：{error}");
                    console.write_str(&report);
                    hk_test::exit(ExitCode::Failure(2));
                }
            }
        }
        Err(error) => {
            let mut report = String::new();
            let _ = writeln!(report, "启动失败：{error}");
            console.write_str(&report);
            hk_test::exit(ExitCode::Failure(1));
        }
    }
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
