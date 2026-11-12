//! AP 启动与停泊：由 Limine MP 协议进入 long mode，在线计数在内核池中维护。

use alloc::format;
use alloc::string::String;
use core::cell::UnsafeCell;
use core::sync::atomic::Ordering;

use hk_boot::limine::ApCpu;
use hk_hal::MAX_CPUS;
use hk_platform_x86_64::cpu;
use hk_smp::AP_POOL;

/// 每个 AP 的调度栈大小；Limine 提供的引导栈太小，深层启动路径会溢出。
const AP_STACK_SIZE: usize = 256 * 1024;

/// 16 字节对齐的 AP 栈；用 `UnsafeCell` 保证落入可写 `.bss` 而不是只读段。
#[repr(align(16))]
struct ApStack(UnsafeCell<[u8; AP_STACK_SIZE]>);

// SAFETY: 每个 CPU 只写自己的栈，且栈在 CPU 运行期间独占。
unsafe impl Sync for ApStack {}

/// 每 CPU 的调度栈；BSP 使用内核启动栈，AP 在入口切换到自己的栈。
static AP_STACKS: [ApStack; MAX_CPUS] =
    [const { ApStack(UnsafeCell::new([0; AP_STACK_SIZE])) }; MAX_CPUS];

fn ap_stack_top(cpu: usize) -> u64 {
    let stack = &AP_STACKS[cpu.min(MAX_CPUS - 1)];
    stack.0.get() as u64 + AP_STACK_SIZE as u64
}

/// 启动 AP、等待其初始化完成，并发送一次 IPI 自检。
pub fn start() -> Result<(usize, usize), String> {
    let Some(_info) = hk_boot::limine::smp_info() else {
        return Ok((0, 0));
    };

    let started = hk_boot::limine::start_aps(ap_entry);
    let timeout = match hk_platform_x86_64::timing::tsc_frequency() {
        0 => 1_000_000_000,
        frequency => frequency,
    };
    let online = AP_POOL.wait(started, hk_platform_x86_64::timing::rdtsc, timeout);
    if online < started {
        return Err(format!("AP 启动超时：{online}/{started}"));
    }

    let ids = AP_POOL.ids();
    if let Some(&apic_id) = ids.first() {
        if apic_id > u32::from(u8::MAX) {
            return Err(format!("APIC 编号超出 xAPIC 范围：{apic_id}"));
        }

        let before = cpu::ipi_count();
        cpu::send_ipi(apic_id as u8, cpu::IPI_VECTOR);
        let begin = hk_platform_x86_64::timing::rdtsc();
        while cpu::ipi_count() == before
            && hk_platform_x86_64::timing::rdtsc().wrapping_sub(begin) < timeout
        {
            core::hint::spin_loop();
        }
        if cpu::ipi_count() == before {
            return Err(String::from("IPI 自检超时"));
        }
    }

    Ok((started, online))
}

/// 验证空闲 AP 能被 IPI 唤醒，并完成一次 TLB shootdown。
pub fn verify_idle_and_tlb(started: usize) -> Result<(), String> {
    if started == 0 {
        return Ok(());
    }

    let timeout = match hk_platform_x86_64::timing::tsc_frequency() {
        0 => 1_000_000_000,
        frequency => frequency,
    };
    if !wait_until(timeout, || cpu::idle_cpus() >= started) {
        return Err(format!("AP 空闲等待超时：{}/{started}", cpu::idle_cpus()));
    }

    let before = cpu::ipi_count();
    cpu::wake_aps();
    if !wait_until(timeout, || cpu::ipi_count() >= before + started) {
        return Err(String::from("AP 唤醒 IPI 超时"));
    }

    if !cpu::shootdown_tlb(Some(0x1000)) {
        return Err(String::from("TLB shootdown 超时"));
    }
    Ok(())
}

fn wait_until(timeout: u64, mut done: impl FnMut() -> bool) -> bool {
    let begin = hk_platform_x86_64::timing::rdtsc();
    while !done() {
        if hk_platform_x86_64::timing::rdtsc().wrapping_sub(begin) >= timeout {
            return false;
        }
        core::hint::spin_loop();
    }
    true
}

unsafe extern "C" fn ap_entry(info: *mut ApCpu) -> ! {
    // SAFETY: Limine 传入的 CPU 描述在入口期间有效；`extra` 由 `start_aps` 写入。
    let (lapic_id, cpu_index) = unsafe {
        let cpu = &*info;
        (cpu.lapic_id, cpu.extra.load(Ordering::Acquire) as usize)
    };
    // Limine 的 AP 引导栈容量不足；先切到静态调度栈，再执行 GDT/IDT 与调度循环。
    switch_ap_stack(cpu_index, lapic_id)
}

/// 在静态栈上继续 AP 初始化；不返回。
fn switch_ap_stack(cpu_index: usize, lapic_id: u32) -> ! {
    let top = ap_stack_top(cpu_index);
    // SAFETY: 栈顶来自本 CPU 的静态数组，16 字节对齐且不会与其它 CPU 重叠；
    // 入口函数签名为 `extern "C" fn(u64, u64) -> !`。
    unsafe {
        core::arch::asm!(
            "mov rsp, {top}",
            "sub rsp, 8",
            "xor ebp, ebp",
            "mov rdi, {lapic}",
            "mov rsi, {cpu}",
            "jmp {entry}",
            top = in(reg) top,
            lapic = in(reg) u64::from(lapic_id),
            cpu = in(reg) cpu_index as u64,
            entry = sym ap_main,
            options(noreturn),
        );
    }
}

/// AP 主循环；运行在静态调度栈上。
extern "C" fn ap_main(lapic_id: u64, cpu_index: u64) -> ! {
    let cpu_index = cpu_index as usize;
    cpu::init_ap(cpu_index);
    cpu::start_ap_tick();
    AP_POOL.mark_online(lapic_id as u32);
    crate::runtime::run_ap(cpu_index)
}
