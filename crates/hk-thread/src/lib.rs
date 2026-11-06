//! 用户态线程库。
//!
//! 线程共享进程地址空间与句柄快照；栈从任务 Arena 分配。入口函数在独立线程上
//! 运行，退出时由内核回收 TCB，[`JoinHandle::join`] 等待其结束。

#![no_std]

extern crate alloc;

use alloc::boxed::Box;

use hk_async::SleepTicks;
use hk_server_api::{alloc, exit, thread_create, thread_join, thread_set_tls};

/// 最小线程栈尺寸。
pub const MIN_STACK: usize = 16 * 1024;

/// 新线程的返回值。
pub struct JoinHandle {
    thread: u64,
}

impl JoinHandle {
    /// 返回线程编号。
    pub const fn id(&self) -> u64 {
        self.thread
    }

    /// 等待线程退出，返回退出码。
    pub fn join(self) -> u64 {
        thread_join(self.thread)
    }
}

/// 以裸入口函数创建线程。
///
/// `entry` 在独立线程上运行，参数为 `argument` 的低 64 位；入口不返回。
pub fn spawn_raw(
    entry: extern "C" fn(u64) -> !,
    argument: u64,
    stack_size: usize,
) -> Option<JoinHandle> {
    let size = stack_size.max(MIN_STACK);
    let stack = alloc(size, 16);
    if stack.is_null() {
        return None;
    }
    spawn_at(entry, stack as u64 + size as u64, argument)
}

/// 用调用方提供的栈创建线程；栈必须常驻到线程退出。
pub fn spawn_at(
    entry: extern "C" fn(u64) -> !,
    stack_top: u64,
    argument: u64,
) -> Option<JoinHandle> {
    let top = (stack_top & !0xF).checked_sub(8)?;
    let thread = thread_create(entry as *const () as u64, top, argument, 0);
    (thread != u64::MAX).then_some(JoinHandle { thread })
}

/// 以闭包创建线程；闭包在独立线程上执行一次。
pub fn spawn<F>(stack_size: usize, entry: F) -> Option<JoinHandle>
where
    F: FnOnce() + Send + 'static,
{
    let boxed: Box<dyn FnOnce() + Send> = Box::new(entry);
    let pointer = Box::into_raw(Box::new(boxed));
    match spawn_raw(trampoline, pointer as u64, stack_size) {
        Some(handle) => Some(handle),
        None => {
            // SAFETY: 线程未创建，指针仍归本函数所有。
            unsafe { drop(Box::from_raw(pointer)) };
            None
        }
    }
}

/// 闭包线程入口。
extern "C" fn trampoline(pointer: u64) -> ! {
    // SAFETY: 指针来自 `spawn` 的 `Box::into_raw`，只在这里回收一次。
    let entry = unsafe { Box::from_raw(pointer as *mut Box<dyn FnOnce() + Send>) };
    entry();
    exit(0)
}

/// 休眠指定调度 tick 数。
pub fn sleep_ticks(ticks: u64) {
    hk_async::block_on(SleepTicks::new(ticks));
}

/// 设置当前线程的 TLS 基址。
pub fn set_tls(tls: u64) -> u64 {
    thread_set_tls(tls)
}

/// 让出 CPU。
pub fn yield_now() {
    hk_server_api::yield_now();
}
