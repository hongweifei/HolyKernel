//! `ax-sync` 的宿主运行时提供者。
//!
//! 服务是单线程用户态任务，但仍可能在任务上下文与 IRQ 处理之间共享状态；
//! 这里用原子自旋实现 spin/rwlock，并让上下文操作保持无副作用。

use core::panic::Location;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use ax_sync::interface::{
    AcquireResult, ContextOps, ContextState, LockMetadata, LockdepOps, RwLockOps, SpinOps,
};

const WRITER: usize = 1 << (usize::BITS - 1);

struct SpinProvider;
struct RwProvider;
struct ContextProvider;
struct LockdepProvider;

#[ax_crate_interface::impl_interface]
impl SpinOps for SpinProvider {
    fn acquire(
        locked: &AtomicBool,
        _metadata: &LockMetadata,
        _lock_addr: usize,
        _context: u8,
        _subclass: u32,
        _caller: &'static Location<'static>,
    ) -> ContextState {
        while locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        ContextState::new(0, 0)
    }

    fn try_acquire(
        locked: &AtomicBool,
        _metadata: &LockMetadata,
        _lock_addr: usize,
        _context: u8,
        _subclass: u32,
        _caller: &'static Location<'static>,
    ) -> AcquireResult {
        let acquired = locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok();
        AcquireResult::new(acquired, ContextState::new(0, 0))
    }

    fn release(
        locked: &AtomicBool,
        _lock_addr: usize,
        _context: u8,
        _context_state: ContextState,
    ) {
        locked.store(false, Ordering::Release);
    }

    fn force_release(locked: &AtomicBool, _lock_addr: usize, _context: u8) {
        locked.store(false, Ordering::Release);
    }

    fn is_locked(locked: &AtomicBool) -> bool {
        locked.load(Ordering::Acquire)
    }
}

#[ax_crate_interface::impl_interface]
impl RwLockOps for RwProvider {
    fn acquire(
        state: &AtomicUsize,
        _metadata: &LockMetadata,
        _lock_addr: usize,
        _context: u8,
        mode: u8,
        _caller: &'static Location<'static>,
    ) -> ContextState {
        if mode == ax_sync::interface::LOCK_MODE_READ {
            loop {
                let current = state.load(Ordering::Relaxed);
                if current & WRITER != 0 {
                    core::hint::spin_loop();
                    continue;
                }
                if state
                    .compare_exchange_weak(
                        current,
                        current + 1,
                        Ordering::Acquire,
                        Ordering::Relaxed,
                    )
                    .is_ok()
                {
                    break;
                }
            }
        } else {
            while state
                .compare_exchange(0, WRITER, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
            {
                core::hint::spin_loop();
            }
        }
        ContextState::new(0, 0)
    }

    fn try_acquire(
        state: &AtomicUsize,
        _metadata: &LockMetadata,
        _lock_addr: usize,
        _context: u8,
        mode: u8,
        _caller: &'static Location<'static>,
    ) -> AcquireResult {
        let acquired = if mode == ax_sync::interface::LOCK_MODE_READ {
            let current = state.load(Ordering::Relaxed);
            current & WRITER == 0
                && state
                    .compare_exchange(
                        current,
                        current + 1,
                        Ordering::Acquire,
                        Ordering::Relaxed,
                    )
                    .is_ok()
        } else {
            state
                .compare_exchange(0, WRITER, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        };
        AcquireResult::new(acquired, ContextState::new(0, 0))
    }

    fn release(
        state: &AtomicUsize,
        _lock_addr: usize,
        _context: u8,
        _context_state: ContextState,
        mode: u8,
    ) {
        if mode == ax_sync::interface::LOCK_MODE_READ {
            state.fetch_sub(1, Ordering::Release);
        } else {
            state.store(0, Ordering::Release);
        }
    }

    fn force_read_decrement(state: &AtomicUsize, _lock_addr: usize, _context: u8) {
        state.fetch_sub(1, Ordering::Release);
    }
}

#[ax_crate_interface::impl_interface]
impl ContextOps for ContextProvider {
    fn enter(_context: u8) -> ContextState {
        ContextState::new(0, 0)
    }

    fn exit(_context: u8, _state: ContextState) {}

    fn irq_return_preempt_enter() -> usize {
        0
    }

    fn irq_return_preempt_exit(_state: usize) {}

    fn hardirq_enter() {}

    fn hardirq_exit() {}
}

#[ax_crate_interface::impl_interface]
impl LockdepOps for LockdepProvider {
    fn set_trace_enabled(_enabled: bool) {}

    fn dump_trace() {}
}
