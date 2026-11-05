//! x86_64 用户态运行时适配：用户现场与平台副作用。
//!
//! 任务表、系统调用、调度循环与启动装配都在 `crates/`；本模块只把
//! `InterruptFrame`、PCI/计时/清零与 Arena 布局接入运行时宿主。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use hk_hal::Console;
use hk_platform_x86_64::cpu::{self, FaultAction, InterruptFrame, SyscallAction};
use hk_platform_x86_64::timing;
use hk_platform_x86_64::{COM1, Serial, USER_VSPACES};
use hk_root_server::{RuntimeHost, ServiceSpec, ServiceTable, Switch, TaskInfo, UserPlatform};

/// 看门狗两次快照之间的墙钟间隔。
const WATCHDOG_INTERVAL_MS: u64 = 3_000;

static WATCHDOG_LAST_MS: AtomicU64 = AtomicU64::new(0);

/// Arena 虚拟基址。
const ARENA_BASE: u64 = 0x0200_0000;

/// 相邻任务 Arena 的间隔。
const ARENA_STRIDE: u64 = 0x20_0000;

/// 每个任务的 Arena 大小。
const ARENA_SIZE: u64 = 2 * 1024 * 1024;

/// 用户可访问地址上限。
const USER_LIMIT: u64 = 0x0000_0080_0000_0000;

fn user_range(address: u64, length: usize) -> bool {
    address >= 0x1000
        && address
            .checked_add(length as u64)
            .is_some_and(|end| end <= USER_LIMIT)
}

struct X86Platform;

impl UserPlatform for X86Platform {
    fn timer_ticks(&self) -> u64 {
        cpu::uptime_ticks()
    }

    fn civil_time(&self) -> Option<hk_hal::CivilTime> {
        hk_platform_x86_64::rtc::civil_time()
    }

    fn boot_cmdline(&self) -> Option<&'static str> {
        hk_boot::cmdline()
    }

    fn pci_read(&self, bus: u8, device: u8, function: u8, offset: u8) -> u32 {
        hk_platform_x86_64::pci::config_read(bus, device, function, offset)
    }

    fn pci_write(&self, bus: u8, device: u8, function: u8, offset: u8, value: u32) {
        hk_platform_x86_64::pci::config_write(bus, device, function, offset, value);
    }

    fn map_mmio(&self, slot: usize, virt: u64, phys: u64, len: usize) -> bool {
        USER_VSPACES.map_mmio(slot, virt, phys, len)
    }

    fn map_dma(&self, slot: usize, virt: u64, phys: u64, len: usize) -> bool {
        USER_VSPACES.map_dma(slot, virt, phys, len)
    }

    fn unmap_dma(&self, slot: usize, virt: u64, len: usize) -> bool {
        USER_VSPACES.unmap_dma(slot, virt, len)
    }

    fn take_irqs(&self, pending: &mut [u64; 4]) {
        *pending = hk_platform_x86_64::irq::take_pending();
    }

    fn console_read(&self) -> Option<u8> {
        hk_platform_x86_64::keyboard::read().or_else(|| Serial::new(COM1).try_read_byte())
    }

    fn take_console_input(&self) -> bool {
        hk_platform_x86_64::keyboard::take_input()
    }

    fn interrupt_controller(&self) -> Option<&dyn hk_hal::InterruptController> {
        Some(&hk_platform_x86_64::bootstrap::CONTROLLER)
    }

    fn iommu(&self) -> Option<&dyn hk_hal::Iommu> {
        Some(&hk_platform_x86_64::iommu::IOMMU)
    }

    fn cache_ops(&self) -> Option<&dyn hk_hal::CacheOps> {
        Some(&hk_platform_x86_64::cache::CACHE)
    }

    fn dma_window(&self, len: usize) -> Option<hk_hal::PhysRange> {
        hk_platform_x86_64::dma::allocate(len)
    }

    fn dma_free(&self, range: hk_hal::PhysRange) {
        hk_platform_x86_64::dma::free(range);
    }

    fn zero(&self, address: u64, length: usize) {
        // SAFETY: Arena 页已映射到当前用户页表，内核经同一页表写入。
        unsafe {
            core::ptr::write_bytes(address as *mut u8, 0, length);
        }
    }

    fn read_user(&self, address: u64, buffer: &mut [u8]) -> bool {
        if buffer.is_empty() {
            return true;
        }
        if !user_range(address, buffer.len()) {
            return false;
        }
        // SAFETY: 地址已校验落在用户空间，且当前 CR3 属于调用任务。
        unsafe {
            core::ptr::copy_nonoverlapping(address as *const u8, buffer.as_mut_ptr(), buffer.len());
        }
        true
    }

    fn write_user(&self, address: u64, data: &[u8]) -> bool {
        if data.is_empty() {
            return true;
        }
        if !user_range(address, data.len()) {
            return false;
        }
        // SAFETY: 地址已校验落在用户空间，且当前 CR3 属于调用任务。
        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr(), address as *mut u8, data.len());
        }
        true
    }

    fn arena_layout(&self, slot: usize) -> (u64, u64) {
        (ARENA_BASE + slot as u64 * ARENA_STRIDE, ARENA_SIZE)
    }

    fn activate(&self, slot: usize) {
        USER_VSPACES.activate(slot);
    }

    fn vspace(&self, slot: usize) -> u64 {
        USER_VSPACES.root(slot)
    }

    fn fork(&self, parent: usize, child: usize) -> bool {
        USER_VSPACES.fork(parent, child)
    }

    fn timestamp(&self) -> u64 {
        hk_platform_x86_64::timing::rdtsc()
    }

    fn tsc_frequency(&self) -> u64 {
        hk_platform_x86_64::timing::tsc_frequency()
    }

    fn wake_aps(&self) {
        cpu::wake_aps();
    }
}

type Runtime = RuntimeHost<InterruptFrame, X86Platform>;

static RUNTIME: Runtime = RuntimeHost::new();

/// 返回任务槽位的 Arena 基址与大小。
pub fn arena(slot: usize) -> (u64, u64) {
    X86Platform.arena_layout(slot)
}

/// 按服务清单装配用户态运行时。
pub fn init(specs: &[ServiceSpec]) -> Result<(), String> {
    RUNTIME.boot(specs, X86Platform, |_index, endpoint| {
        cpu::user_frame(0, 0, endpoint)
    })?;

    cpu::set_syscall_hook(dispatch);
    cpu::set_preempt_hook(preempt);
    cpu::set_fault_hook(fault);
    Ok(())
}

/// 设置任务的用户态初始现场。
pub fn attach(index: usize, context: InterruptFrame) {
    RUNTIME.attach(index, context);
}

/// 生成任务视图快照。
pub fn tasks() -> Vec<TaskInfo> {
    RUNTIME.tasks()
}

/// 返回最近的内核 trace 事件。
pub fn trace(limit: usize) -> Vec<hk_root_server::TraceEvent> {
    RUNTIME.trace(limit)
}

/// 返回运行时统计。
pub fn stats() -> hk_root_server::RuntimeStats {
    RUNTIME.stats()
}

/// 调度至多 `steps` 步；返回是否已无可运行线程。
pub fn run_budgeted(table: &mut ServiceTable, steps: usize) -> bool {
    cpu::start_tick();
    let idle = RUNTIME.run_cpu_budget(0, Some(table), steps);
    cpu::stop_tick();
    if !idle && watchdog_enabled() {
        watchdog_snapshot();
    }
    idle
}

/// 是否通过 `watchdog` 启动参数开启活性看门狗。
fn watchdog_enabled() -> bool {
    hk_boot::cmdline().is_some_and(|line| line.contains("watchdog"))
}

/// 系统未空闲但持续运行时，周期性输出任务快照，便于定位活性停顿。
fn watchdog_snapshot() {
    let frequency = timing::tsc_frequency();
    if frequency == 0 {
        return;
    }
    let now = ((u128::from(timing::rdtsc()) * 1000) / u128::from(frequency)) as u64;
    let last = WATCHDOG_LAST_MS.load(Ordering::Relaxed);
    if now.saturating_sub(last) < WATCHDOG_INTERVAL_MS {
        return;
    }
    WATCHDOG_LAST_MS.store(now, Ordering::Relaxed);

    let console = Serial::new(COM1);
    let stats = RUNTIME.stats();
    console.write_str(&format!(
        "[watchdog] t={now}ms resumes={:?}\n",
        stats.resumes
    ));
    for task in RUNTIME.tasks() {
        console.write_str(&format!(
            "[watchdog] {} tcb={} state={} pending={} handles={}\n",
            task.name, task.tcb, task.state, task.pending, task.handles
        ));
    }
}

/// AP 调度循环：无任务时关闭 tick 并等待唤醒 IPI。
pub fn run_ap(cpu: usize) -> ! {
    loop {
        RUNTIME.run_cpu(cpu, None);
        cpu::stop_tick();
        cpu::idle(cpu);
        cpu::start_ap_tick();
    }
}

/// 按重启策略监督服务；返回重启数量。
pub fn supervise(loader: &mut crate::shell::Loader<'_>, table: &mut ServiceTable) -> usize {
    hk_root_server::service::supervise(
        &RUNTIME,
        table,
        loader,
        hk_root_server::service::restart_policy,
    )
}

/// 同步其他 CPU 记录的服务退出。
pub fn drain_exits(table: &mut ServiceTable) {
    RUNTIME.drain_exits(table);
}

/// 启动服务。
pub fn start_service(
    loader: &mut crate::shell::Loader<'_>,
    services: &mut ServiceTable,
    name: &str,
    parent: Option<&str>,
) -> String {
    hk_root_server::service::start(&RUNTIME, services, loader, name, parent)
}

/// 重启已退出的服务。
pub fn restart_service(
    loader: &mut crate::shell::Loader<'_>,
    services: &mut ServiceTable,
    name: &str,
    parent: Option<&str>,
) -> String {
    hk_root_server::service::restart(&RUNTIME, services, loader, name, parent)
}

fn dispatch(frame: &mut InterruptFrame) -> SyscallAction {
    match RUNTIME.dispatch(cpu::cpu_index(), frame) {
        Switch::Continue => SyscallAction::Continue,
        Switch::Block => SyscallAction::Block,
        Switch::Exit => SyscallAction::Exit,
    }
}

fn preempt(frame: &InterruptFrame) -> bool {
    RUNTIME.preempt(cpu::cpu_index(), *frame)
}

fn fault(frame: &mut InterruptFrame) -> FaultAction {
    if frame.vector == 14
        && frame.error & 0b11 == 0b11
        && hk_platform_x86_64::paging::resolve_cow(cpu::fault_address())
    {
        return FaultAction::Resume;
    }

    let console = Serial::new(COM1);
    console.write_str(&format!(
        "用户异常：{}（CPU {}）cr2={:#x} rip={:#x}\n",
        cpu::exception_name(frame.vector),
        cpu::cpu_index(),
        cpu::fault_address(),
        frame.rip
    ));
    RUNTIME.fault(cpu::cpu_index(), 0xE000 | frame.vector);
    FaultAction::Exit
}
