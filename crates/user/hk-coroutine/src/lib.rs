//! 用户态有栈协程。
//!
//! 每个协程持有自己的栈与保存的执行现场；[`Coroutine::resume`] 从调用方栈切到
//! 协程栈，[`suspend`] 再切回调用方。切换只保存 callee-saved 寄存器与栈指针，
//! 不经过内核，适合把等待点组织成顺序代码。
//!
//! 同一时刻只有一个协程在某个线程上运行；跨线程并发使用请让每个线程各自持有
//! 协程集合。

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicPtr, Ordering};

#[cfg(target_arch = "x86_64")]
mod x86_64;

#[cfg(not(target_arch = "x86_64"))]
compile_error!("hk-coroutine 当前只支持 x86_64");

/// 协程状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// 已创建或已挂起，可以继续执行。
    Suspended,
    /// 入口函数已返回，协程不会再执行。
    Finished,
}

/// 最小栈尺寸。
const MIN_STACK: usize = 4096;

/// 保存在协程栈上的现场数量：r15、r14、r12、r13、rbx、rbp 与返回地址。
const INITIAL_SLOTS: usize = 7;

/// 入口指针在初始栈中的槽位。
const ENTRY_SLOT: usize = 6;

/// 协程入口参数寄存器对应的槽位（r12）。
const ARGUMENT_SLOT: usize = 3;

/// 当前线程正在运行的协程。
static CURRENT: AtomicPtr<Control> = AtomicPtr::new(core::ptr::null_mut());

/// 协程自身的控制块；放在堆上以保证地址稳定。
struct Control {
    /// 挂起时的栈指针。
    context: u64,
    /// 当前状态。
    state: State,
    /// 入口函数；执行一次后取走。
    entry: Option<Box<dyn FnOnce()>>,
    /// 本次恢复的调用方现场指针。
    caller: *mut u64,
}

/// 有栈协程。
pub struct Coroutine {
    control: Box<Control>,
    /// 协程栈；字段只为持有分配。
    _stack: Vec<u8>,
}

impl Coroutine {
    /// 以指定栈尺寸与入口函数创建协程；栈过小或分配失败时返回 `None`。
    pub fn new<F>(stack_size: usize, entry: F) -> Option<Self>
    where
        F: FnOnce() + 'static,
    {
        let size = stack_size.max(MIN_STACK);
        let stack = vec![0_u8; size];
        let mut control = Box::new(Control {
            context: 0,
            state: State::Suspended,
            entry: Some(Box::new(entry)),
            caller: core::ptr::null_mut(),
        });

        let base = stack.as_ptr() as usize;
        let top = base.checked_add(size)? & !0xF;
        let sp = top.checked_sub(INITIAL_SLOTS * core::mem::size_of::<u64>())?;
        if sp < base {
            return None;
        }

        // SAFETY: `sp` 落在本协程栈内，按 8 字节对齐；写入的槽位都在栈顶以下。
        unsafe {
            let slots = sp as *mut u64;
            for index in 0..INITIAL_SLOTS {
                slots.add(index).write(0);
            }
            slots
                .add(ARGUMENT_SLOT)
                .write(core::ptr::addr_of!(*control) as u64);
            slots.add(ENTRY_SLOT).write(x86_64::entry_address());
        }
        control.context = sp as u64;

        Some(Self {
            control,
            _stack: stack,
        })
    }

    /// 恢复协程执行，直到它挂起或结束。
    pub fn resume(&mut self) -> State {
        let control = core::ptr::addr_of_mut!(*self.control);
        // SAFETY: `control` 指向 `self` 持有的稳定堆分配，直到本协程被释放。
        unsafe {
            if (*control).state == State::Finished {
                return State::Finished;
            }
            let mut caller = 0_u64;
            (*control).caller = core::ptr::addr_of_mut!(caller);
            let previous = CURRENT.swap(control, Ordering::AcqRel);
            let context = (*control).context;
            x86_64::switch(core::ptr::addr_of_mut!(caller), context);
            CURRENT.store(previous, Ordering::Release);
            (*control).state
        }
    }

    /// 返回当前状态。
    pub fn state(&self) -> State {
        self.control.state
    }

    /// 是否已经结束。
    pub fn is_finished(&self) -> bool {
        self.control.state == State::Finished
    }
}

/// 挂起当前协程并回到恢复它的调用方；不在协程内调用时无操作。
pub fn suspend() {
    let current = CURRENT.load(Ordering::Acquire);
    if current.is_null() {
        return;
    }
    // SAFETY: `current` 只指向仍在执行的协程控制块；读取与写入都在切换前完成。
    unsafe {
        let caller = (*current).caller;
        if caller.is_null() || (*current).state == State::Finished {
            return;
        }
        (*current).state = State::Suspended;
        x86_64::switch(core::ptr::addr_of_mut!((*current).context), *caller);
    }
}

/// 协程入口：执行闭包，标记结束并切回调用方。
///
/// # Safety
/// 由汇编入口以 `r12` 传入当前协程的控制块指针，调用方保证指针有效。
#[unsafe(no_mangle)]
unsafe extern "C" fn hk_coroutine_entry(control: *mut Control) -> ! {
    // SAFETY: 指针来自 `Coroutine::new` 写入初始栈的控制块，协程存活期间有效。
    unsafe {
        let entry = (*control).entry.take();
        if let Some(entry) = entry {
            entry();
        }
        (*control).state = State::Finished;
        let caller = (*control).caller;
        if !caller.is_null() {
            x86_64::switch(core::ptr::addr_of_mut!((*control).context), *caller);
        }
    }
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    static ORDER: AtomicU32 = AtomicU32::new(0);
    static FIRST: AtomicU32 = AtomicU32::new(0);
    static SECOND: AtomicU32 = AtomicU32::new(0);
    static NESTED: AtomicU32 = AtomicU32::new(0);
    static OUTER: AtomicU32 = AtomicU32::new(0);

    /// 当前协程指针是线程级状态；测试串行执行，避免互相覆盖。
    static TEST_LOCK: AtomicBool = AtomicBool::new(false);

    struct Serial;

    impl Serial {
        fn new() -> Self {
            while TEST_LOCK.swap(true, Ordering::Acquire) {
                core::hint::spin_loop();
            }
            Self
        }
    }

    impl Drop for Serial {
        fn drop(&mut self) {
            TEST_LOCK.store(false, Ordering::Release);
        }
    }

    #[test]
    fn runs_until_suspend_and_finish() {
        let _serial = Serial::new();
        ORDER.store(0, Ordering::Relaxed);
        let mut coroutine = Coroutine::new(16 * 1024, || {
            ORDER.fetch_add(1, Ordering::Relaxed);
            suspend();
            ORDER.fetch_add(10, Ordering::Relaxed);
        })
        .expect("创建协程");

        assert_eq!(coroutine.state(), State::Suspended);
        assert_eq!(coroutine.resume(), State::Suspended);
        assert_eq!(ORDER.load(Ordering::Relaxed), 1);
        assert_eq!(coroutine.resume(), State::Finished);
        assert_eq!(ORDER.load(Ordering::Relaxed), 11);
        assert_eq!(coroutine.resume(), State::Finished);
        assert!(coroutine.is_finished());
    }

    #[test]
    fn interleaves_two_coroutines() {
        let _serial = Serial::new();
        FIRST.store(0, Ordering::Relaxed);
        SECOND.store(0, Ordering::Relaxed);
        let mut first = Coroutine::new(16 * 1024, || {
            FIRST.fetch_add(1, Ordering::Relaxed);
            suspend();
            FIRST.fetch_add(100, Ordering::Relaxed);
        })
        .expect("创建第一个协程");
        let mut second = Coroutine::new(16 * 1024, || {
            SECOND.fetch_add(10, Ordering::Relaxed);
            suspend();
            SECOND.fetch_add(1000, Ordering::Relaxed);
        })
        .expect("创建第二个协程");

        assert_eq!(first.resume(), State::Suspended);
        assert_eq!(second.resume(), State::Suspended);
        assert_eq!(FIRST.load(Ordering::Relaxed), 1);
        assert_eq!(SECOND.load(Ordering::Relaxed), 10);
        assert_eq!(first.resume(), State::Finished);
        assert_eq!(second.resume(), State::Finished);
        assert_eq!(FIRST.load(Ordering::Relaxed), 101);
        assert_eq!(SECOND.load(Ordering::Relaxed), 1010);
    }

    #[test]
    fn nested_resume_restores_current() {
        let _serial = Serial::new();
        NESTED.store(0, Ordering::Relaxed);
        OUTER.store(0, Ordering::Relaxed);
        let mut inner = Coroutine::new(16 * 1024, || {
            NESTED.fetch_add(1, Ordering::Relaxed);
            suspend();
            NESTED.fetch_add(10, Ordering::Relaxed);
        })
        .expect("创建内层协程");
        let mut outer = Coroutine::new(32 * 1024, move || {
            OUTER.fetch_add(1, Ordering::Relaxed);
            assert_eq!(inner.resume(), State::Suspended);
            suspend();
            assert_eq!(inner.resume(), State::Finished);
            OUTER.fetch_add(10, Ordering::Relaxed);
        })
        .expect("创建外层协程");

        assert_eq!(outer.resume(), State::Suspended);
        assert_eq!(NESTED.load(Ordering::Relaxed), 1);
        assert_eq!(OUTER.load(Ordering::Relaxed), 1);
        assert_eq!(outer.resume(), State::Finished);
        assert_eq!(NESTED.load(Ordering::Relaxed), 11);
        assert_eq!(OUTER.load(Ordering::Relaxed), 11);
    }

    #[test]
    fn suspend_outside_coroutine_is_noop() {
        let _serial = Serial::new();
        suspend();
    }
}
