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
mod allocator;
#[cfg(target_os = "none")]
mod host;
#[cfg(target_os = "none")]
mod loader;
#[cfg(target_os = "none")]
mod memory;
#[cfg(target_os = "none")]
mod shell;
#[cfg(target_os = "none")]
mod smp;

#[cfg(target_os = "none")]
use alloc::format;
#[cfg(target_os = "none")]
use alloc::string::String;
#[cfg(target_os = "none")]
use core::fmt::Write;
#[cfg(target_os = "none")]
use core::panic::PanicInfo;

#[cfg(target_os = "none")]
use hk_arch_x86_64::bootstrap;
#[cfg(target_os = "none")]
use hk_arch_x86_64::{COM1, PageTable, Serial};
#[cfg(target_os = "none")]
use hk_hal::ExitCode;
#[cfg(target_os = "none")]
use hk_hal::{Console, Platform};
#[cfg(target_os = "none")]
use hk_root_server::ServiceTable;

/// 内核启动栈字节数。
///
/// 装载器与运行时的调试构建需要数百 KiB 栈；Limine 的入口栈容量不保证，
/// 且可能与启动器的页表同处保留内存。内核从自己的 `.bss` 栈启动，
/// 只把启动器栈用于最初的几条指令。
#[cfg(target_os = "none")]
const BOOT_STACK_SIZE: usize = 1024 * 1024;

#[cfg(target_os = "none")]
core::arch::global_asm!(
    ".section .bss",
    ".balign 16",
    ".global hk_boot_stack",
    "hk_boot_stack:",
    ".skip {size}",
    ".global hk_boot_stack_top",
    "hk_boot_stack_top:",
    ".section .text.entry",
    ".global _start",
    "_start:",
    "lea rsp, [rip + hk_boot_stack_top]",
    "and rsp, -16",
    "xor rbp, rbp",
    "call kmain",
    "ud2",
    size = const BOOT_STACK_SIZE,
);

#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
extern "C" fn kmain() -> ! {
    arm_boot_canary();
    let boot_start = hk_arch_x86_64::timing::rdtsc();
    let console = Serial::new(COM1);
    console.init();
    console.write_str("HolyKernel x86_64\n");
    hk_arch_x86_64::cpu::init();

    let frequency = hk_arch_x86_64::timing::calibrate();
    match frequency {
        Some(hz) => {
            console.write_str(&format!("TSC 频率：{} MHz\n", hz / 1_000_000));
            hk_arch_x86_64::rtc::init(hz);
        }
        None => console.write_str("TSC 校准失败\n"),
    }

    let info = match hk_boot::limine::boot_info() {
        Ok(info) => info,
        Err(error) => {
            announce(&console, None, &format!("启动失败：{error}\n"));
            hk_hal::exit(ExitCode::Failure(1));
        }
    };

    let screen = bootstrap::framebuffer_console(&info);
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
    announce(&console, screen.as_ref(), &format!("{info}"));

    if let Err(error) = hk_arch_x86_64::platform::PLATFORM.init(&info) {
        announce(&console, screen.as_ref(), &format!("{error}\n"));
        hk_hal::exit(ExitCode::Failure(2));
    }

    announce(&console, screen.as_ref(), "启动信息校验通过\n");
    host::set_framebuffer(info.framebuffer);

    let direct_map = match info.direct_map {
        Some(direct_map) => direct_map,
        None => fail(
            &console,
            screen.as_ref(),
            "启动信息缺少 direct map\n",
            ExitCode::Failure(3),
        ),
    };
    let direct_map_base = direct_map.virt_base.get();
    let memory = match memory::bring_up(&info, direct_map_base) {
        Ok(memory) => memory,
        Err(error) => fail(
            &console,
            screen.as_ref(),
            &format!("{error}\n"),
            ExitCode::Failure(3),
        ),
    };
    announce(&console, screen.as_ref(), &format!("{memory}\n"));
    let paging = PageTable::new(direct_map_base);

    if !hk_arch_x86_64::cpu::init_stacks(&paging, &hk_arch_x86_64::paging::FRAME_BANK) {
        fail(
            &console,
            screen.as_ref(),
            "内核栈 guard page 初始化失败\n",
            ExitCode::Failure(3),
        );
    }
    hk_arch_x86_64::system_arena::register_kernel_mappings(&paging);

    if let Err(error) = bootstrap::init_interrupts(&paging, direct_map_base) {
        fail(
            &console,
            screen.as_ref(),
            &format!("中断初始化失败：{error}\n"),
            ExitCode::Failure(3),
        );
    }
    announce(&console, screen.as_ref(), "本地 APIC 就绪\n");

    if let Some(hz) = frequency {
        hk_arch_x86_64::cpu::calibrate_tick(hz);
    }

    match bootstrap::init_ioapic(&info, &paging, direct_map_base) {
        Ok(Some(ioapic)) => {
            let _ = bootstrap::init_keyboard(&ioapic);
            announce(
                &console,
                screen.as_ref(),
                &format!(
                    "IOAPIC 就绪：GSI {} → 向量 {}（目标 APIC {}）\n",
                    ioapic.gsi, ioapic.vector, ioapic.destination
                ),
            );
            announce(
                &console,
                screen.as_ref(),
                &format!("本地 APIC：{} 个\n", ioapic.cpus),
            );
        }
        Ok(None) => announce(&console, screen.as_ref(), "未找到 IOAPIC，键盘中断不可用\n"),
        Err(error) => fail(
            &console,
            screen.as_ref(),
            &format!("IOAPIC 初始化失败：{error}\n"),
            ExitCode::Failure(3),
        ),
    }
    let numa = bootstrap::apply_numa_topology();
    announce(
        &console,
        screen.as_ref(),
        &format!("NUMA 物理记录：{numa}\n"),
    );
    if let Some(phys) = hk_arch_x86_64::system::verify_numa_node() {
        announce(
            &console,
            screen.as_ref(),
            &format!("NUMA 节点分配 ok：节点 1 物理 {phys:#x}\n"),
        );
    }
    hk_arch_x86_64::system_arena::register_kernel_mappings(&paging);

    match smp::start() {
        Ok((started, online)) => {
            let ipi = hk_arch_x86_64::cpu::ipi_count();
            announce(
                &console,
                screen.as_ref(),
                &format!("AP 在线：{online} 个（启动 {started}，IPI {ipi} 次）\n"),
            );
            if let Err(error) = smp::verify_idle_and_tlb(started) {
                fail(
                    &console,
                    screen.as_ref(),
                    &format!("SMP 空闲唤醒失败：{error}\n"),
                    ExitCode::Failure(3),
                );
            }
            announce(
                &console,
                screen.as_ref(),
                &format!(
                    "AP 空闲 IPI 唤醒完成，TLB shootdown {} 次\n",
                    hk_arch_x86_64::cpu::tlb_shootdowns()
                ),
            );
        }
        Err(error) => fail(
            &console,
            screen.as_ref(),
            &format!("SMP 启动失败：{error}\n"),
            ExitCode::Failure(3),
        ),
    }

    report_time(
        &console,
        screen.as_ref(),
        "内核初始化耗时",
        boot_start,
        frequency,
    );

    match hk_boot::limine::cmdline() {
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
            hk_arch_x86_64::cpu::arm_timer(200);
            while !hk_arch_x86_64::cpu::timer_finished() {
                // SAFETY: `hlt` 等到下一次定时器中断。
                unsafe {
                    core::arch::asm!("hlt", options(nomem, nostack));
                }
            }
            hk_arch_x86_64::cpu::disarm_timer();
        }
        _ => {
            announce(&console, screen.as_ref(), "加载服务并进入用户态\n");
            let mut services = ServiceTable::new();
            if let Err(error) = loader::run(
                &info,
                direct_map_base,
                &hk_arch_x86_64::paging::FRAME_BANK,
                &paging,
                &mut services,
            ) {
                fail(
                    &console,
                    screen.as_ref(),
                    &format!("服务加载失败：{error}\n"),
                    ExitCode::Failure(4),
                );
            }
            announce(&console, screen.as_ref(), "服务进入空闲\n");
            announce(
                &console,
                screen.as_ref(),
                &format!("调度 tick：{} 次\n", hk_arch_x86_64::cpu::timer_ticks()),
            );
            report_time(&console, screen.as_ref(), "启动耗时", boot_start, frequency);

            let mut shell_loader = shell::Loader {
                info: &info,
                direct_map_base,
                allocator: &hk_arch_x86_64::paging::FRAME_BANK,
                paging: &paging,
            };

            let interactive =
                hk_boot::limine::cmdline().is_some_and(|line| line.contains("shell-interactive"));
            if interactive {
                if let Err(error) =
                    shell::interactive(&console, screen.as_ref(), &mut services, &mut shell_loader)
                {
                    fail(
                        &console,
                        screen.as_ref(),
                        &format!("shell 失败：{error}\n"),
                        ExitCode::Failure(5),
                    );
                }
            } else if let Some(line) = hk_boot::limine::cmdline()
                && line.contains("shell-test")
            {
                let script: alloc::vec::Vec<&str> = line
                    .split_once("shell-test")
                    .map(|(_, rest)| {
                        rest.split(';')
                            .map(str::trim)
                            .filter(|segment| !segment.is_empty())
                            .collect()
                    })
                    .unwrap_or_default();
                if let Err(error) = shell::run(
                    &console,
                    screen.as_ref(),
                    &mut services,
                    &mut shell_loader,
                    &script,
                ) {
                    fail(
                        &console,
                        screen.as_ref(),
                        &format!("shell 失败：{error}\n"),
                        ExitCode::Failure(5),
                    );
                }
            }
        }
    }

    hk_hal::exit(ExitCode::Success);
}

#[cfg(target_os = "none")]
fn announce(console: &Serial, screen: Option<&hk_arch_x86_64::FramebufferConsole>, text: &str) {
    console.write_str(text);
    if let Some(screen) = screen {
        screen.write_str(text);
    }
}

#[cfg(target_os = "none")]
fn report_time(
    console: &Serial,
    screen: Option<&hk_arch_x86_64::FramebufferConsole>,
    label: &str,
    start: u64,
    frequency: Option<u64>,
) {
    if let Some(frequency) = frequency {
        let elapsed = hk_arch_x86_64::timing::elapsed_ms(start, frequency);
        announce(console, screen, &format!("{label}：{elapsed} ms\n"));
    }
}

#[cfg(target_os = "none")]
fn fail(
    console: &Serial,
    screen: Option<&hk_arch_x86_64::FramebufferConsole>,
    message: &str,
    code: ExitCode,
) -> ! {
    announce(console, screen, message);
    hk_hal::exit(code)
}

#[cfg(target_os = "none")]
fn run_timer_test(console: &Serial) -> ! {
    console.write_str("启动本地 APIC 定时器，等待 5 次中断\n");

    hk_arch_x86_64::cpu::arm_timer(5);

    while !hk_arch_x86_64::cpu::timer_finished() {
        // SAFETY: `hlt` 等到下一次中断；中断由本地 APIC 定时器投递。
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }

    hk_arch_x86_64::cpu::disarm_timer();
    let ticks = hk_arch_x86_64::cpu::timer_ticks();
    let mut report = String::new();
    let _ = writeln!(report, "定时器中断 {ticks} 次");
    console.write_str(&report);

    hk_hal::exit(ExitCode::Success)
}

#[cfg(target_os = "none")]
struct SerialWriter(Serial);

#[cfg(target_os = "none")]
impl core::fmt::Write for SerialWriter {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        self.0.write_str(text);
        Ok(())
    }
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    let mut console = SerialWriter(Serial::new(COM1));
    let _ = writeln!(console, "\n内核 panic：{info}");
    let (low, high) = boot_stack_range();
    let rsp: u64;
    // SAFETY: 只读取当前栈指针。
    unsafe {
        core::arch::asm!(
            "mov {}, rsp",
            out(reg) rsp,
            options(nomem, nostack, preserves_flags)
        );
    }
    let _ = writeln!(
        console,
        "  启动栈 {low:#018x}..{high:#018x} rsp={rsp:#018x} 栈内={} 金丝雀={}",
        rsp >= low && rsp < high,
        if boot_canary_intact() { "正常" } else { "已被覆盖" }
    );
    match crate::smp::ap_stack_clobbered() {
        Some(cpu) => {
            let _ = writeln!(console, "  AP 栈金丝雀被覆盖：cpu={cpu}");
        }
        None => {
            let _ = writeln!(console, "  AP 栈金丝雀=正常");
        }
    }
    // 堆范围与 chunk 统计：坏释放/空洞别名发生时，先确认地址是否落在内核堆内。
    // 坏释放 panic 时堆锁可能仍被现场持有，这里用 try_lock，读不到就标明。
    let _ = writeln!(
        console,
        "  内核堆={:?} chunk={:?} 最近写监视={:?}",
        hk_arch_x86_64::paging::kernel_heap(),
        crate::allocator::ALLOCATOR.chunk_stats_try(),
        hk_arch_x86_64::cpu::last_watch_hit()
    );
    hk_arch_x86_64::cpu::report_panic_context();
    // 最近的大块释放：坏释放/节点提前归还时，这里的返回地址指向释放路径。
    let _ = writeln!(console, "  最近释放（新→旧）：");
    for record in hk_kernel::allocator::free_log::recent(8) {
        if record[1] == 0 {
            continue;
        }
        let _ = writeln!(
            console,
            "    {:#018x}+{:#x} ret={:#018x} {:#018x} {:#018x}",
            record[0], record[1], record[2], record[3], record[4]
        );
    }
    let _ = writeln!(console, "  最近分配（新→旧）：");
    for record in hk_kernel::allocator::alloc_log::recent(12) {
        if record[1] == 0 {
            continue;
        }
        let _ = writeln!(
            console,
            "    {:#018x}+{:#x} ret={:#018x} {:#018x} {:#018x}",
            record[0], record[1], record[2], record[3], record[4]
        );
    }
    hk_hal::exit(ExitCode::Failure(70))
}

/// 启动栈底部的金丝雀长度与取值。
#[cfg(target_os = "none")]
const BOOT_CANARY_BYTES: usize = 64;
#[cfg(target_os = "none")]
const BOOT_CANARY: u8 = 0xA5;

#[cfg(target_os = "none")]
unsafe extern "C" {
    static hk_boot_stack: u8;
    static hk_boot_stack_top: u8;
}

/// 启动栈的 `[低端, 高端)` 区间。
#[cfg(target_os = "none")]
fn boot_stack_range() -> (u64, u64) {
    (
        core::ptr::addr_of!(hk_boot_stack) as u64,
        core::ptr::addr_of!(hk_boot_stack_top) as u64,
    )
}

/// 在启动栈最低端写入金丝雀；栈向下溢出到静态区时这里最先被覆盖。
#[cfg(target_os = "none")]
fn arm_boot_canary() {
    let (low, _) = boot_stack_range();
    // SAFETY: 启动栈底部 64 字节在本函数运行时不会被使用（栈从顶端向下增长）。
    unsafe {
        core::ptr::write_bytes(low as *mut u8, BOOT_CANARY, BOOT_CANARY_BYTES);
    }
}

/// 启动栈金丝雀是否完好。
#[cfg(target_os = "none")]
fn boot_canary_intact() -> bool {
    let (low, _) = boot_stack_range();
    (0..BOOT_CANARY_BYTES).all(|offset| {
        // SAFETY: 金丝雀区间位于启动栈内，读取有效。
        unsafe { core::ptr::read_volatile((low as *const u8).add(offset)) == BOOT_CANARY }
    })
}

#[cfg(not(target_os = "none"))]
fn main() {
    eprintln!("hk-image-x86_64-qemu 只在 x86_64-unknown-none 目标上构建");
}
