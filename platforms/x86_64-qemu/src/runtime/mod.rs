//! x86_64 用户态运行时适配：用户现场、平台副作用与启动装配。
//!
//! 任务表、系统调用语义与 Arena 记账位于 `hk-kernel::user`；本模块只提供
//! `InterruptFrame` 的现场操作、PCI/计时/清零适配，并把启动期服务装配成运行时。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use hk_hal::UserContext;
use hk_kernel::user::{Step, Switch, UserPlatform, UserRuntime};
use hk_platform_x86_64::cpu::{self, InterruptFrame, SyscallAction};
use hk_root_server::{ServiceTable, TaskInfo};
use spin::Mutex;

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

type Runtime = UserRuntime<InterruptFrame, X86Platform>;

static RUNTIME: Mutex<Option<Runtime>> = Mutex::new(None);

/// 返回任务槽位的 Arena 基址与大小。
pub fn arena(slot: usize) -> (u64, u64) {
    X86Platform.arena_layout(slot)
}

/// 按服务清单装配用户态运行时。
pub fn init(names: &[&'static str]) -> Result<(), String> {
    let runtime = hk_root_server::boot::user_runtime(names, X86Platform, |_index, endpoint| {
        cpu::user_frame(0, 0, endpoint)
    })
    .map_err(|error| format!("{error}"))?;

    *RUNTIME.lock() = Some(runtime);
    cpu::set_syscall_hook(dispatch);
    cpu::set_preempt_hook(preempt);
    Ok(())
}

/// 设置任务的用户态初始现场。
pub fn attach(index: usize, context: InterruptFrame) {
    if let Some(runtime) = RUNTIME.lock().as_mut() {
        runtime.attach(index, context);
    }
}

/// 返回已登记任务数量。
pub fn task_count() -> usize {
    RUNTIME.lock().as_ref().map_or(0, Runtime::task_count)
}

/// 按名称返回任务的 TCB 编号。
pub fn tcb_of(name: &str) -> Option<u64> {
    RUNTIME
        .lock()
        .as_ref()
        .and_then(|runtime| runtime.tcb_of(name))
}

/// 返回 Root 线程编号，作为引导期服务的父任务。
pub fn root() -> u64 {
    RUNTIME.lock().as_ref().map_or(0, Runtime::root)
}

/// 装载新任务，返回分配的端点编号。
pub fn start(name: &str, context: InterruptFrame, parent: Option<u64>) -> Result<u64, String> {
    let mut guard = RUNTIME.lock();
    let runtime = guard
        .as_mut()
        .ok_or_else(|| String::from("运行时未初始化"))?;
    runtime
        .start(name, context, parent)
        .map_err(|error| format!("{error}"))
}

/// 生成任务视图快照。
pub fn tasks() -> Vec<TaskInfo> {
    RUNTIME
        .lock()
        .as_ref()
        .map_or_else(Vec::new, Runtime::tasks)
}

/// 依次调度任务直到没有可运行线程。
pub fn run(table: &mut ServiceTable) {
    cpu::start_tick();
    loop {
        let step = RUNTIME.lock().as_mut().map(|runtime| runtime.step());
        match step {
            Some(Step::Resume { index, context }) => {
                // 恢复用户态前释放运行时锁；系统调用与抢占回调会重新获取。
                context.resume();
                let exited = RUNTIME
                    .lock()
                    .as_mut()
                    .and_then(|runtime| runtime.finish(index));
                if let Some((name, code)) = exited {
                    table.mark_exited(&name, code);
                }
            }
            Some(Step::Blocked) => {}
            Some(Step::Done) | None => break,
        }
    }
    cpu::stop_tick();
}

fn dispatch(frame: &mut InterruptFrame) -> SyscallAction {
    let mut guard = RUNTIME.lock();
    let Some(runtime) = guard.as_mut() else {
        frame.rax = u64::MAX;
        return SyscallAction::Continue;
    };

    match runtime.dispatch(frame) {
        Switch::Continue => SyscallAction::Continue,
        Switch::Block => SyscallAction::Block,
        Switch::Exit => SyscallAction::Exit,
    }
}

fn preempt(frame: &InterruptFrame) -> bool {
    let mut guard = RUNTIME.lock();
    let Some(runtime) = guard.as_mut() else {
        return false;
    };
    runtime.preempt(*frame)
}
