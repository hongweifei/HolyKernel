//! x86_64 用户态运行时适配：用户现场与平台副作用。
//!
//! 任务表、系统调用、调度循环与启动装配都在 `crates/`；本模块只把
//! `InterruptFrame`、PCI/计时/清零与 Arena 布局接入运行时宿主。

use alloc::string::String;
use alloc::vec::Vec;

use hk_platform_x86_64::cpu::{self, InterruptFrame, SyscallAction};
use hk_root_server::{RuntimeHost, ServiceTable, Switch, TaskInfo, UserPlatform};

/// Arena 虚拟基址。
const ARENA_BASE: u64 = 0x0100_0000;

/// 相邻任务 Arena 的间隔。
const ARENA_STRIDE: u64 = 0x2_0000;

/// 每个任务的 Arena 大小。
const ARENA_SIZE: u64 = 64 * 1024;

struct X86Platform;

impl UserPlatform for X86Platform {
    fn timer_ticks(&self) -> u64 {
        cpu::timer_ticks()
    }

    fn pci_read(&self, bus: u8, device: u8, function: u8, offset: u8) -> u32 {
        hk_platform_x86_64::pci::config_read(bus, device, function, offset)
    }

    fn zero(&self, address: u64, length: usize) {
        // SAFETY: Arena 页已映射为用户可写；内核与用户共享同一页表。
        unsafe {
            core::ptr::write_bytes(address as *mut u8, 0, length);
        }
    }

    fn arena_layout(&self, slot: usize) -> (u64, u64) {
        (ARENA_BASE + slot as u64 * ARENA_STRIDE, ARENA_SIZE)
    }
}

type Runtime = RuntimeHost<InterruptFrame, X86Platform>;

static RUNTIME: Runtime = RuntimeHost::new();

/// 返回任务槽位的 Arena 基址与大小。
pub fn arena(slot: usize) -> (u64, u64) {
    X86Platform.arena_layout(slot)
}

/// 按服务清单装配用户态运行时。
pub fn init(names: &[&'static str]) -> Result<(), String> {
    RUNTIME.boot(names, X86Platform, |_index, endpoint| {
        cpu::user_frame(0, 0, endpoint)
    })?;

    cpu::set_syscall_hook(dispatch);
    cpu::set_preempt_hook(preempt);
    Ok(())
}

/// 设置任务的用户态初始现场。
pub fn attach(index: usize, context: InterruptFrame) {
    RUNTIME.attach(index, context);
}

/// 返回已登记任务数量。
pub fn task_count() -> usize {
    RUNTIME.task_count()
}

/// 按名称返回任务的 TCB 编号。
pub fn tcb_of(name: &str) -> Option<u64> {
    RUNTIME.tcb_of(name)
}

/// 返回 Root 线程编号，作为引导期服务的父任务。
pub fn root() -> u64 {
    RUNTIME.root()
}

/// 装载新任务，返回分配的端点编号。
pub fn start(name: &str, context: InterruptFrame, parent: Option<u64>) -> Result<u64, String> {
    RUNTIME.start(name, context, parent)
}

/// 生成任务视图快照。
pub fn tasks() -> Vec<TaskInfo> {
    RUNTIME.tasks()
}

/// 依次调度任务直到没有可运行线程。
pub fn run(table: &mut ServiceTable) {
    cpu::start_tick();
    RUNTIME.run(table);
    cpu::stop_tick();
}

fn dispatch(frame: &mut InterruptFrame) -> SyscallAction {
    match RUNTIME.dispatch(frame) {
        Switch::Continue => SyscallAction::Continue,
        Switch::Block => SyscallAction::Block,
        Switch::Exit => SyscallAction::Exit,
    }
}

fn preempt(frame: &InterruptFrame) -> bool {
    RUNTIME.preempt(*frame)
}
