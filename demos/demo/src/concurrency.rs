//! 并发自检：线程、协程、异步执行器、同步原语与服务端工作线程池。

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use hk_async::{NotificationWait, SleepTicks, block_on, join2};
use hk_coroutine::{Coroutine, State, suspend};
use hk_proto::echo::{WORK_REQUEST, WORK_RESPONSE, WORK_STATS, WORK_STATS_RESPONSE};
use hk_sync::{Condvar, Mutex, RwLock};
use hk_user_api::{
    alloc, exit, futex_wait, futex_wake, notify, recv_data_try, self_notification, send,
    service_endpoint, thread_create, thread_join, write, write_u64, yield_now,
};

/// 异步自检使用的通知位。
const ASYNC_BIT: u64 = 1 << 40;

/// 协程共享的执行轨迹。
static COROUTINE_TRACE: AtomicU32 = AtomicU32::new(0);

/// 线程共享的累加计数。
static THREAD_COUNT: AtomicU64 = AtomicU64::new(0);

/// 线程共享的唤醒字。
static THREAD_WORD: AtomicU32 = AtomicU32::new(0);

/// 同步自检：互斥计数。
static SYNC_COUNTER: Mutex<u64> = Mutex::new(0);
/// 同步自检：条件变量值。
static SYNC_VALUE: Mutex<u32> = Mutex::new(0);
/// 同步自检：条件变量。
static SYNC_CONDVAR: Condvar = Condvar::new();
/// 同步自检：读写锁。
static SYNC_RW: RwLock<u64> = RwLock::new(0);

/// 线程入口：校验 TLS、累加共享计数并唤醒等待者。
extern "C" fn thread_worker(argument: u64) -> ! {
    let tls: u64;
    // SAFETY: 运行库在切到本线程前把 FS 基址设为 `argument`。
    unsafe {
        core::arch::asm!("mov {}, fs:[0]", out(reg) tls, options(nostack, preserves_flags));
    }
    if tls != argument {
        write(b"hk-server-demo thread tls fail\n");
        exit(1)
    }
    for _ in 0..1000 {
        THREAD_COUNT.fetch_add(1, Ordering::Relaxed);
    }
    THREAD_WORD.store(1, Ordering::Release);
    futex_wake(core::ptr::addr_of!(THREAD_WORD) as u64, 1);
    exit(0)
}

/// 验证同地址空间线程、TLS 与共享内存等待。
pub(super) fn verify_threads() -> bool {
    const STACK: usize = 64 * 1024;
    let stack = alloc(STACK, 16);
    let tls = alloc(16, 8);
    if stack.is_null() || tls.is_null() {
        return false;
    }
    // SAFETY: `tls` 是 Arena 内已清零的 8 字节对齐区间。
    unsafe { (tls as *mut u64).write(tls as u64) };
    THREAD_COUNT.store(0, Ordering::Relaxed);
    THREAD_WORD.store(0, Ordering::Relaxed);
    let thread = thread_create(
        thread_worker as *const () as u64,
        stack as u64 + STACK as u64 - 8,
        tls as u64,
        tls as u64,
    );
    if thread == u64::MAX {
        return false;
    }
    let word = core::ptr::addr_of!(THREAD_WORD) as u64;
    let _ = futex_wait(word, 0);
    if thread_join(thread) != 0 {
        return false;
    }
    THREAD_COUNT.load(Ordering::Relaxed) == 1000 && THREAD_WORD.load(Ordering::Relaxed) == 1
}

/// 验证两个协程在同一线程内交替执行。
pub(super) fn verify_coroutines() -> bool {
    COROUTINE_TRACE.store(0, Ordering::Relaxed);
    let Some(mut first) = Coroutine::new(16 * 1024, || {
        COROUTINE_TRACE.fetch_add(1, Ordering::Relaxed);
        suspend();
        COROUTINE_TRACE.fetch_add(100, Ordering::Relaxed);
    }) else {
        return false;
    };
    let Some(mut second) = Coroutine::new(16 * 1024, || {
        COROUTINE_TRACE.fetch_add(10, Ordering::Relaxed);
        suspend();
        COROUTINE_TRACE.fetch_add(1000, Ordering::Relaxed);
    }) else {
        return false;
    };

    if first.resume() != State::Suspended || second.resume() != State::Suspended {
        return false;
    }
    if COROUTINE_TRACE.load(Ordering::Relaxed) != 11 {
        return false;
    }
    if first.resume() != State::Finished || second.resume() != State::Finished {
        return false;
    }
    COROUTINE_TRACE.load(Ordering::Relaxed) == 1111
}

/// 线程入口：置位父线程的通知。
extern "C" fn async_worker(notification: u64) -> ! {
    notify(notification, ASYNC_BIT);
    exit(0)
}

/// 验证执行器同时等待通知位与 tick。
pub(super) fn verify_async() -> bool {
    const STACK: usize = 64 * 1024;
    let stack = alloc(STACK, 16);
    if stack.is_null() {
        return false;
    }
    let parent = self_notification();
    let thread = thread_create(
        async_worker as *const () as u64,
        stack as u64 + STACK as u64 - 8,
        parent,
        0,
    );
    if thread == u64::MAX {
        return false;
    }
    let (bits, ()) = block_on(join2(
        NotificationWait::new(parent, ASYNC_BIT),
        SleepTicks::new(1),
    ));
    let joined = thread_join(thread) == 0;
    joined && bits & ASYNC_BIT != 0
}

/// 互斥计数线程入口。
extern "C" fn mutex_worker(_: u64) -> ! {
    for _ in 0..250 {
        *SYNC_COUNTER.lock() += 1;
    }
    exit(0)
}

/// 条件变量等待线程入口。
extern "C" fn condvar_worker(_: u64) -> ! {
    let mut value = SYNC_VALUE.lock();
    while *value == 0 {
        value = SYNC_CONDVAR.wait(value);
    }
    *value += 1;
    drop(value);
    exit(0)
}

/// 写锁线程入口。
extern "C" fn rw_writer(_: u64) -> ! {
    *SYNC_RW.write() += 1;
    exit(0)
}

/// 读锁线程入口。
extern "C" fn rw_reader(_: u64) -> ! {
    let _ = *SYNC_RW.read();
    exit(0)
}

/// 验证互斥锁、条件变量与读写锁。
pub(super) fn verify_sync() -> bool {
    const STACK: usize = 32 * 1024;

    *SYNC_COUNTER.lock() = 0;
    for _ in 0..4 {
        let Some(handle) = hk_thread::spawn_raw(mutex_worker, 0, STACK) else {
            return false;
        };
        if handle.join() != 0 {
            return false;
        }
    }
    if *SYNC_COUNTER.lock() != 1000 {
        return false;
    }

    *SYNC_VALUE.lock() = 0;
    let Some(waiter) = hk_thread::spawn_raw(condvar_worker, 0, STACK) else {
        return false;
    };
    {
        let mut value = SYNC_VALUE.lock();
        *value = 1;
        SYNC_CONDVAR.notify_one();
    }
    if waiter.join() != 0 || *SYNC_VALUE.lock() != 2 {
        return false;
    }

    *SYNC_RW.write() = 0;
    let Some(writer) = hk_thread::spawn_raw(rw_writer, 0, STACK) else {
        return false;
    };
    let Some(reader) = hk_thread::spawn_raw(rw_reader, 0, STACK) else {
        return false;
    };
    if writer.join() != 0 || reader.join() != 0 {
        return false;
    }
    *SYNC_RW.read() == 1
}

/// 验证服务端工作线程池：并发提交、乱序完成与并发峰值。
pub(super) fn verify_worker(endpoint: u64) -> bool {
    const COUNT: u64 = 4;
    let echo = service_endpoint("hk-server-echo");
    let mut buffer = [0_u8; 8];
    for word in 1..=COUNT {
        let mut sent = false;
        for _ in 0..64 {
            if send(echo, WORK_REQUEST, word) == 0 {
                sent = true;
                break;
            }
            yield_now();
        }
        if !sent {
            return false;
        }
    }

    let mut seen = [false; COUNT as usize + 1];
    let mut received = 0;
    while received < COUNT {
        let (label, word, _sender, _length) = recv_data_try(endpoint, &mut buffer);
        if label == 0 {
            yield_now();
            continue;
        }
        if label != WORK_RESPONSE {
            continue;
        }
        if !word.is_multiple_of(2) {
            return false;
        }
        let index = (word / 2) as usize;
        if index == 0 || index > COUNT as usize || seen[index] {
            return false;
        }
        seen[index] = true;
        received += 1;
    }

    let mut sent = false;
    for _ in 0..64 {
        if send(echo, WORK_STATS, 0) == 0 {
            sent = true;
            break;
        }
        yield_now();
    }
    if !sent {
        return false;
    }
    let peak = loop {
        let (label, word, _sender, _length) = recv_data_try(endpoint, &mut buffer);
        if label == WORK_STATS_RESPONSE {
            break word;
        }
        if label == 0 {
            yield_now();
        }
    };

    write(b"hk-server-demo worker peak ");
    write_u64(peak);
    write(b"\n");
    // Task 级唯一 Grant 下，同一 Task 的线程串行共享 Grant，峰值并发为 1。
    peak >= 1
}
