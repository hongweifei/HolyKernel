//! `crab-usb` 的 HolyKernel 宿主适配。
//!
//! 提供 `KernelOp` 的延迟实现，以及把 USB 驱动的 future 推进到完成的最小
//! 事件循环；xHCI 事件在任务上下文由 `EventHandler` 排空。

#![cfg_attr(target_os = "none", no_std)]

extern crate alloc;

use core::future::Future;
use core::pin::pin;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use core::time::Duration;

pub use crab_usb;

mod sync;
use crab_usb::{EventHandler, KernelOp};
use hk_server_api::sleep_ticks;

/// 调度 tick 的毫秒数。
const TICK_MS: u128 = 10;

/// 供 `crab-usb` 使用的内核操作。
pub struct HostKernel;

impl KernelOp for HostKernel {
    fn delay(&self, duration: Duration) {
        let millis = duration.as_millis();
        let ticks = millis.div_ceil(TICK_MS).max(1);
        let ticks = u64::try_from(ticks).unwrap_or(u64::MAX);
        sleep_ticks(ticks);
    }
}

static KERNEL: HostKernel = HostKernel;

/// 返回全局 `KernelOp`。
pub fn kernel() -> &'static dyn KernelOp {
    &KERNEL
}

/// 推进一个 future 直到完成。
///
/// 每次 future 返回 `Pending` 时排空一次 xHCI 事件，然后等待 IRQ 通知。
pub fn block_on<F: Future>(future: F, notification: u64, events: &EventHandler) -> F::Output {
    let mut future = pin!(future);
    let waker = noop_waker();
    let mut context = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => {
                events.handle_event();
                let _ = hk_server_api::wait_notify(notification, 1);
            }
        }
    }
}

fn noop_waker() -> Waker {
    unsafe fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(core::ptr::null(), &VTABLE)
    }
    unsafe fn wake(_: *const ()) {}
    unsafe fn wake_by_ref(_: *const ()) {}
    unsafe fn drop(_: *const ()) {}

    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop);
    // SAFETY: vtable 的四个函数均不访问数据指针。
    unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &VTABLE)) }
}
