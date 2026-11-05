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
mod loader;
#[cfg(target_os = "none")]
mod runtime;
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
use hk_hal::Console;
#[cfg(target_os = "none")]
use hk_platform_x86_64::bootstrap;
#[cfg(target_os = "none")]
use hk_platform_x86_64::{COM1, FrameAllocator, PageTable, Serial};
#[cfg(target_os = "none")]
use hk_root_server::ServiceTable;
#[cfg(target_os = "none")]
use hk_test::ExitCode;

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
    let boot_start = hk_platform_x86_64::timing::rdtsc();
    let console = Serial::new(COM1);
    console.init();
    console.write_str("HolyKernel x86_64\n");
    hk_platform_x86_64::cpu::init();

    let frequency = hk_platform_x86_64::timing::calibrate();
    match frequency {
        Some(hz) => {
            console.write_str(&format!("TSC 频率：{} MHz\n", hz / 1_000_000));
            hk_platform_x86_64::rtc::init(hz);
        }
        None => console.write_str("TSC 校准失败\n"),
    }

    let info = match hk_boot::boot_info() {
        Ok(info) => info,
        Err(error) => {
            announce(&console, None, &format!("启动失败：{error}\n"));
            hk_test::exit(ExitCode::Failure(1));
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

    if let Err(error) = info.validate() {
        announce(
            &console,
            screen.as_ref(),
            &format!("启动信息校验失败：{error}\n"),
        );
        hk_test::exit(ExitCode::Failure(2));
    }

    announce(&console, screen.as_ref(), "启动信息校验通过\n");
    runtime::set_framebuffer(info.framebuffer);

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
    let mut allocator = match FrameAllocator::new(info.memory_map, info.kernel_image) {
        Some(allocator) => allocator,
        None => fail(
            &console,
            screen.as_ref(),
            "没有可用的物理内存区域\n",
            ExitCode::Failure(3),
        ),
    };
    let paging = PageTable::new(direct_map_base);
    hk_platform_x86_64::paging::init_runtime(direct_map_base);
    hk_platform_x86_64::cache::init(direct_map_base);
    hk_platform_x86_64::paging::reserve_frames(&mut allocator, 512);
    hk_platform_x86_64::paging::reserve_user_table_frames(&mut allocator, 64);
    if let Some(dma) = allocator.allocate_contiguous(4 * 1024 * 1024) {
        hk_platform_x86_64::dma::init(dma);
    }

    if let Err(error) = bootstrap::init_interrupts(&paging, &mut allocator, direct_map_base) {
        fail(
            &console,
            screen.as_ref(),
            &format!("中断初始化失败：{error}\n"),
            ExitCode::Failure(3),
        );
    }
    announce(&console, screen.as_ref(), "本地 APIC 就绪\n");

    if let Some(hz) = frequency {
        hk_platform_x86_64::cpu::calibrate_tick(hz);
    }

    match bootstrap::init_ioapic(&info, &paging, &mut allocator, direct_map_base) {
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

    match smp::start() {
        Ok((started, online)) => {
            let ipi = hk_platform_x86_64::cpu::ipi_count();
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
                    hk_platform_x86_64::cpu::tlb_shootdowns()
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
        _ => {
            announce(&console, screen.as_ref(), "加载服务并进入用户态\n");
            let mut services = ServiceTable::new();
            if let Err(error) = loader::run(
                &info,
                direct_map_base,
                &mut allocator,
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
                &format!("调度 tick：{} 次\n", hk_platform_x86_64::cpu::timer_ticks()),
            );
            report_time(&console, screen.as_ref(), "启动耗时", boot_start, frequency);

            let mut shell_loader = shell::Loader {
                info: &info,
                direct_map_base,
                allocator: &mut allocator,
                paging: &paging,
            };

            let interactive =
                hk_boot::cmdline().is_some_and(|line| line.contains("shell-interactive"));
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
            } else if let Some(line) = hk_boot::cmdline() {
                if line.contains("shell-test") {
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
    }

    hk_test::exit(ExitCode::Success);
}

#[cfg(target_os = "none")]
fn announce(console: &Serial, screen: Option<&hk_platform_x86_64::FramebufferConsole>, text: &str) {
    console.write_str(text);
    if let Some(screen) = screen {
        screen.write_str(text);
    }
}

#[cfg(target_os = "none")]
fn report_time(
    console: &Serial,
    screen: Option<&hk_platform_x86_64::FramebufferConsole>,
    label: &str,
    start: u64,
    frequency: Option<u64>,
) {
    if let Some(frequency) = frequency {
        let elapsed = hk_platform_x86_64::timing::elapsed_ms(start, frequency);
        announce(console, screen, &format!("{label}：{elapsed} ms\n"));
    }
}

#[cfg(target_os = "none")]
fn fail(
    console: &Serial,
    screen: Option<&hk_platform_x86_64::FramebufferConsole>,
    message: &str,
    code: ExitCode,
) -> ! {
    announce(console, screen, message);
    hk_test::exit(code)
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
    hk_test::exit(ExitCode::Failure(70))
}

#[cfg(not(target_os = "none"))]
fn main() {
    eprintln!("hk-x86_64 只在 x86_64-unknown-none 目标上构建");
}
