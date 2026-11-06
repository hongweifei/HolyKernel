//! 服务内工作线程池。
//!
//! 服务的主线程负责收发 IPC；耗时计算交给固定数量的工作线程，完成结果回到队列，
//! 主线程再统一回复。这样既保持“IPC 接收者身份属于主线程”，又让多个请求并行计算。
//!
//! 池本身泄漏到进程生命周期：服务长驻，主线程退出即随进程回收。

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec::Vec;

use core::sync::atomic::{AtomicBool, Ordering};

use hk_sync::{Condvar, Mutex};
use hk_thread::JoinHandle;

/// 工作线程栈尺寸。
const WORKER_STACK: usize = 32 * 1024;

type Job = (u64, Box<dyn FnOnce() -> u64 + Send>);

/// 工作线程池。
pub struct Pool {
    queue: Mutex<VecDeque<Job>>,
    ready: Condvar,
    completed: Mutex<VecDeque<(u64, u64)>>,
    wake: Mutex<Option<(u64, u64)>>,
    stopped: AtomicBool,
    workers: Mutex<Vec<JoinHandle>>,
}

impl Pool {
    /// 启动 `workers` 个工作线程；池随进程常驻。
    ///
    /// 返回 `None` 表示线程创建失败（栈分配或内核拒绝）。
    pub fn start(workers: usize) -> Option<&'static Pool> {
        Self::start_with_stack(workers, WORKER_STACK)
    }

    /// 以指定栈尺寸启动工作线程。
    pub fn start_with_stack(workers: usize, stack_size: usize) -> Option<&'static Pool> {
        if workers == 0 {
            return None;
        }
        let pool: &'static Pool = Box::leak(Box::new(Pool {
            queue: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
            completed: Mutex::new(VecDeque::new()),
            wake: Mutex::new(None),
            stopped: AtomicBool::new(false),
            workers: Mutex::new(Vec::new()),
        }));
        let pointer = pool as *const Pool as u64;
        let mut handles = pool.workers.lock();
        for _ in 0..workers {
            let handle = hk_thread::spawn_raw(worker, pointer, stack_size).or_else(|| {
                // Arena 已满时改用服务堆分配栈；栈随线程常驻到进程退出。
                let size = stack_size.max(hk_thread::MIN_STACK);
                let stack = Vec::leak(alloc::vec![0_u8; size]);
                hk_thread::spawn_at(worker, stack.as_ptr() as u64 + stack.len() as u64, pointer)
            });
            match handle {
                Some(handle) => handles.push(handle),
                None => {
                    drop(handles);
                    pool.shutdown();
                    return None;
                }
            }
        }
        Some(pool)
    }

    /// 提交一个任务；`id` 原样随完成结果返回。
    ///
    /// 返回 `false` 表示池已关闭。
    pub fn submit<F>(&self, id: u64, job: F) -> bool
    where
        F: FnOnce() -> u64 + Send + 'static,
    {
        if self.stopped.load(Ordering::Acquire) {
            return false;
        }
        self.queue.lock().push_back((id, Box::new(job)));
        self.ready.notify_one();
        true
    }

    /// 取出一个已完成任务；没有完成时返回 `None`。
    pub fn try_next(&self) -> Option<(u64, u64)> {
        self.completed.lock().pop_front()
    }

    /// 设置完成唤醒：每个任务完成后向 `endpoint` 发送 `label`，参数为任务 id。
    ///
    /// 服务主线程可以继续阻塞在接收上，由唤醒消息通知它回收完成结果。
    pub fn set_wake(&self, endpoint: u64, label: u64) {
        *self.wake.lock() = Some((endpoint, label));
    }

    /// 关闭池并等待工作线程退出。
    ///
    /// 必须在主线程调用，工作线程内调用会等待自身。
    pub fn shutdown(&self) {
        self.stopped.store(true, Ordering::Release);
        self.ready.notify_all();
        let handles = core::mem::take(&mut *self.workers.lock());
        for handle in handles {
            let _ = handle.join();
        }
    }
}

/// 工作线程入口。
extern "C" fn worker(pointer: u64) -> ! {
    // SAFETY: 指针来自 `Pool::start` 泄漏的池，进程内线程共享同一地址空间。
    let pool = unsafe { &*(pointer as *const Pool) };
    loop {
        let job = {
            let mut queue = pool.queue.lock();
            loop {
                if pool.stopped.load(Ordering::Acquire) {
                    break None;
                }
                if let Some(job) = queue.pop_front() {
                    break Some(job);
                }
                queue = pool.ready.wait(queue);
            }
        };
        let Some((id, job)) = job else {
            break;
        };
        let result = job();
        pool.completed.lock().push_back((id, result));
        let wake = *pool.wake.lock();
        if let Some((endpoint, label)) = wake {
            let _ = hk_server_api::send(endpoint, label, id);
        }
    }
    hk_server_api::exit(0)
}
