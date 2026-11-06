//! 用户态异步执行器。
//!
//! 服务在自己的线程上运行单线程执行器：[`block_on`] 轮询 future，未就绪时让出
//! CPU；[`block_on_with`] 允许调用方替换让出策略。所有 future 只做非阻塞检查
//! （通知位、tick、块完成），阻塞语义仍由内核对象承担。
//!
//! 组合子提供 [`join2`]（同时等待两个 future）与 `async` 块内的普通 `.await`。

#![no_std]

extern crate alloc;

use alloc::sync::Arc;
use alloc::task::Wake;
use core::future::Future;
use core::pin::{Pin, pin};
use core::sync::atomic::{AtomicBool, Ordering};
use core::task::{Context, Poll, Waker};

/// 唤醒标志。
struct Flag {
    woken: AtomicBool,
}

impl Wake for Flag {
    fn wake(self: Arc<Self>) {
        self.woken.store(true, Ordering::Release);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.woken.store(true, Ordering::Release);
    }
}

/// 在当前线程执行 future，未就绪时让出 CPU。
pub fn block_on<F: Future>(future: F) -> F::Output {
    block_on_with(future, || {
        hk_server_api::yield_now();
    })
}

/// 用给定让出函数驱动 future。
///
/// future 未就绪且没有新唤醒时调用 `park`；`park` 返回后立即重新轮询。
pub fn block_on_with<F: Future, P: FnMut()>(future: F, mut park: P) -> F::Output {
    let flag = Arc::new(Flag {
        woken: AtomicBool::new(false),
    });
    let waker = Waker::from(Arc::clone(&flag));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);

    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => {
                if !flag.woken.swap(false, Ordering::AcqRel) {
                    park();
                }
            }
        }
    }
}

/// 同时等待两个 future，两者都就绪后返回结果。
pub fn join2<A: Future, B: Future>(first: A, second: B) -> Join2<A, B> {
    Join2 {
        first: Some(first),
        second: Some(second),
        first_out: None,
        second_out: None,
    }
}

/// [`join2`] 的组合 future。
pub struct Join2<A: Future, B: Future> {
    first: Option<A>,
    second: Option<B>,
    first_out: Option<A::Output>,
    second_out: Option<B::Output>,
}

impl<A: Future, B: Future> Future for Join2<A, B> {
    type Output = (A::Output, B::Output);

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: 组合 future 被 Pin 后字段地址保持稳定；只对仍存在的字段做 Pin 投影。
        let this = unsafe { self.get_unchecked_mut() };

        if this.first_out.is_none()
            && let Some(future) = this.first.as_mut()
        {
            // SAFETY: `first` 位于已 Pin 的 Join2 内，地址不会移动。
            match unsafe { Pin::new_unchecked(future) }.poll(context) {
                Poll::Ready(value) => {
                    this.first_out = Some(value);
                    this.first = None;
                }
                Poll::Pending => {}
            }
        }

        if this.second_out.is_none()
            && let Some(future) = this.second.as_mut()
        {
            // SAFETY: `second` 位于已 Pin 的 Join2 内，地址不会移动。
            match unsafe { Pin::new_unchecked(future) }.poll(context) {
                Poll::Ready(value) => {
                    this.second_out = Some(value);
                    this.second = None;
                }
                Poll::Pending => {}
            }
        }

        match (this.first_out.take(), this.second_out.take()) {
            (Some(first), Some(second)) => Poll::Ready((first, second)),
            (first, second) => {
                this.first_out = first;
                this.second_out = second;
                Poll::Pending
            }
        }
    }
}

/// 等待指定 tick 数。
pub struct SleepTicks {
    ticks: u64,
    started: Option<u64>,
}

impl SleepTicks {
    /// 构造等待 `ticks` 个调度 tick 的 future。
    pub const fn new(ticks: u64) -> Self {
        Self {
            ticks,
            started: None,
        }
    }
}

impl Future for SleepTicks {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        let now = hk_server_api::ticks();
        let started = *this.started.get_or_insert(now);
        if now.wrapping_sub(started) >= this.ticks {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

/// 等待通知位并返回命中的位。
pub struct NotificationWait {
    target: u64,
    mask: u64,
}

impl NotificationWait {
    /// 构造等待 `target` 上 `mask` 位的 future。
    pub const fn new(target: u64, mask: u64) -> Self {
        Self { target, mask }
    }
}

impl Future for NotificationWait {
    type Output = u64;

    fn poll(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<u64> {
        let bits = hk_server_api::wait_notify_try(self.target, self.mask);
        if bits != 0 {
            Poll::Ready(bits)
        } else {
            Poll::Pending
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::pin::Pin;
    use core::task::Poll;

    struct Countdown {
        remaining: u32,
    }

    impl Future for Countdown {
        type Output = u32;

        fn poll(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<u32> {
            if self.remaining == 0 {
                Poll::Ready(0)
            } else {
                self.remaining -= 1;
                Poll::Pending
            }
        }
    }

    struct WakeOnce {
        woken: bool,
    }

    impl Future for WakeOnce {
        type Output = bool;

        fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<bool> {
            if self.woken {
                Poll::Ready(true)
            } else {
                self.woken = true;
                context.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }

    #[test]
    fn drives_future_through_park() {
        let mut parks = 0;
        let value = block_on_with(Countdown { remaining: 3 }, || parks += 1);
        assert_eq!(value, 0);
        assert_eq!(parks, 3);
    }

    #[test]
    fn wake_skips_park() {
        let mut parks = 0;
        let value = block_on_with(WakeOnce { woken: false }, || parks += 1);
        assert!(value);
        assert_eq!(parks, 0);
    }

    #[test]
    fn join_waits_for_both() {
        let (first, second) = block_on_with(join2(async { 1_u32 }, async { 2_u32 }), || {
            panic!("就绪的 future 不应让出")
        });
        assert_eq!((first, second), (1, 2));
    }

    #[test]
    fn join_keeps_polling_unfinished_side() {
        let mut parks = 0;
        let (first, second) =
            block_on_with(join2(async { 7_u32 }, Countdown { remaining: 2 }), || {
                parks += 1
            });
        assert_eq!(first, 7);
        assert_eq!(second, 0);
        assert_eq!(parks, 2);
    }
}
