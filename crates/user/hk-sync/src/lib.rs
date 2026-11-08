//! 用户态同步原语。
//!
//! 互斥量与条件变量建立在共享内存字与 futex 系统调用上：无竞争时只做原子
//! 比较交换，发生竞争才挂起线程。所有对象都可以放在 `static` 里由多个线程
//! 共享。

#![no_std]

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicU32, Ordering};

use hk_syscall_client::{futex_wait, futex_wake};

/// 未加锁。
const UNLOCKED: u32 = 0;
/// 已加锁，无等待者。
const LOCKED: u32 = 1;
/// 已加锁且有等待者。
const CONTENDED: u32 = 2;
/// 进入睡眠前的自旋次数。
const SPIN_LIMIT: u32 = 32;

fn address(word: &AtomicU32) -> u64 {
    core::ptr::addr_of!(*word) as u64
}

fn wait(word: &AtomicU32, expected: u32) {
    let _ = futex_wait(address(word), expected);
}

fn wake(word: &AtomicU32, count: u32) {
    let _ = futex_wake(address(word), count);
}

/// 互斥锁。
pub struct Mutex<T> {
    state: AtomicU32,
    data: UnsafeCell<T>,
}

// SAFETY: 持有守卫期间对数据的访问是排他的；`T: Send` 保证可以跨线程移动。
unsafe impl<T: Send> Sync for Mutex<T> {}

impl<T> Mutex<T> {
    /// 构造未加锁的互斥量。
    pub const fn new(data: T) -> Self {
        Self {
            state: AtomicU32::new(UNLOCKED),
            data: UnsafeCell::new(data),
        }
    }

    /// 获取锁，必要时挂起当前线程。
    pub fn lock(&self) -> MutexGuard<'_, T> {
        self.acquire();
        MutexGuard { mutex: self }
    }

    /// 尝试获取锁；已被占用时返回 `None`。
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        self.state
            .compare_exchange(UNLOCKED, LOCKED, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| MutexGuard { mutex: self })
    }

    /// 返回底层数据的可变引用；调用方需保证没有其它访问。
    pub fn get_mut(&mut self) -> &mut T {
        // SAFETY: `&mut self` 保证没有并发访问。
        unsafe { &mut *self.data.get() }
    }

    fn acquire(&self) {
        if self
            .state
            .compare_exchange(UNLOCKED, LOCKED, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            return;
        }
        let mut spins = 0;
        loop {
            let state = self.state.load(Ordering::Relaxed);
            if state == UNLOCKED {
                if self
                    .state
                    .compare_exchange(UNLOCKED, LOCKED, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    return;
                }
                continue;
            }
            if spins < SPIN_LIMIT {
                spins += 1;
                core::hint::spin_loop();
                continue;
            }
            if state != CONTENDED {
                let _ = self.state.compare_exchange(
                    LOCKED,
                    CONTENDED,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                );
            }
            wait(&self.state, CONTENDED);
            spins = 0;
        }
    }

    fn release(&self) {
        if self.state.swap(UNLOCKED, Ordering::Release) == CONTENDED {
            wake(&self.state, 1);
        }
    }
}

/// 互斥锁守卫。
pub struct MutexGuard<'a, T> {
    mutex: &'a Mutex<T>,
}

impl<T> Deref for MutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: 守卫持有锁，保证排他访问。
        unsafe { &*self.mutex.data.get() }
    }
}

impl<T> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: 守卫持有锁，保证排他访问。
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<T> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        self.mutex.release();
    }
}

impl<T> MutexGuard<'_, T> {
    /// 释放锁、等待 `word` 变化后重新加锁；供条件变量使用。
    fn wait_on(&mut self, word: &AtomicU32, expected: u32) {
        self.mutex.release();
        wait(word, expected);
        self.mutex.acquire();
    }
}

/// 条件变量。
pub struct Condvar {
    sequence: AtomicU32,
}

impl Condvar {
    /// 构造条件变量。
    pub const fn new() -> Self {
        Self {
            sequence: AtomicU32::new(0),
        }
    }

    /// 释放互斥量并等待通知，返回重新加锁后的守卫。
    pub fn wait<'a, T>(&self, mut guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
        let sequence = self.sequence.load(Ordering::Acquire);
        guard.wait_on(&self.sequence, sequence);
        guard
    }

    /// 唤醒一个等待者。
    pub fn notify_one(&self) {
        self.sequence.fetch_add(1, Ordering::Release);
        wake(&self.sequence, 1);
    }

    /// 唤醒全部等待者。
    pub fn notify_all(&self) {
        self.sequence.fetch_add(1, Ordering::Release);
        wake(&self.sequence, u32::MAX);
    }
}

impl Default for Condvar {
    fn default() -> Self {
        Self::new()
    }
}

struct RwState {
    readers: u32,
    writer: bool,
    waiting_writers: u32,
}

/// 读写锁。
pub struct RwLock<T> {
    state: Mutex<RwState>,
    changed: Condvar,
    data: UnsafeCell<T>,
}

// SAFETY: 读守卫共享只读访问，写守卫排他访问；`T: Send` 允许跨线程持有。
unsafe impl<T: Send> Sync for RwLock<T> {}

impl<T> RwLock<T> {
    /// 构造读写锁。
    pub const fn new(data: T) -> Self {
        Self {
            state: Mutex::new(RwState {
                readers: 0,
                writer: false,
                waiting_writers: 0,
            }),
            changed: Condvar::new(),
            data: UnsafeCell::new(data),
        }
    }

    /// 获取读锁；有写者等待时优先让出。
    pub fn read(&self) -> RwLockReadGuard<'_, T> {
        let mut state = self.state.lock();
        while state.writer || state.waiting_writers > 0 {
            state = self.changed.wait(state);
        }
        state.readers += 1;
        drop(state);
        RwLockReadGuard { lock: self }
    }

    /// 获取写锁。
    pub fn write(&self) -> RwLockWriteGuard<'_, T> {
        let mut state = self.state.lock();
        state.waiting_writers += 1;
        while state.writer || state.readers > 0 {
            state = self.changed.wait(state);
        }
        state.waiting_writers -= 1;
        state.writer = true;
        drop(state);
        RwLockWriteGuard { lock: self }
    }

    /// 返回底层数据的可变引用；调用方需保证没有其它访问。
    pub fn get_mut(&mut self) -> &mut T {
        // SAFETY: `&mut self` 保证没有并发访问。
        unsafe { &mut *self.data.get() }
    }
}

/// 读守卫。
pub struct RwLockReadGuard<'a, T> {
    lock: &'a RwLock<T>,
}

impl<T> Deref for RwLockReadGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: 读守卫期间写者被阻塞。
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> Drop for RwLockReadGuard<'_, T> {
    fn drop(&mut self) {
        let mut state = self.lock.state.lock();
        state.readers -= 1;
        if state.readers == 0 {
            drop(state);
            self.lock.changed.notify_all();
        }
    }
}

/// 写守卫。
pub struct RwLockWriteGuard<'a, T> {
    lock: &'a RwLock<T>,
}

impl<T> Deref for RwLockWriteGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: 写守卫排他持有锁。
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for RwLockWriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: 写守卫排他持有锁。
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for RwLockWriteGuard<'_, T> {
    fn drop(&mut self) {
        let mut state = self.lock.state.lock();
        state.writer = false;
        drop(state);
        self.lock.changed.notify_all();
    }
}

/// 一次性初始化。
pub struct Once {
    state: AtomicU32,
}

impl Once {
    /// 构造未执行的初始化器。
    pub const fn new() -> Self {
        Self {
            state: AtomicU32::new(0),
        }
    }

    /// 执行一次初始化；其它线程等待执行完成。
    pub fn call_once<F: FnOnce()>(&self, initialize: F) {
        if self
            .state
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            initialize();
            self.state.store(2, Ordering::Release);
            wake(&self.state, u32::MAX);
            return;
        }
        while self.state.load(Ordering::Acquire) != 2 {
            wait(&self.state, 1);
        }
    }

    /// 是否已经初始化完成。
    pub fn is_completed(&self) -> bool {
        self.state.load(Ordering::Acquire) == 2
    }
}

impl Default for Once {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locks_and_unlocks_without_contention() {
        let mut mutex = Mutex::new(1_u32);
        {
            let mut guard = mutex.lock();
            *guard += 1;
        }
        let guard = mutex.try_lock().expect("可再次加锁");
        assert_eq!(*guard, 2);
        assert!(mutex.try_lock().is_none());
        drop(guard);
        assert!(mutex.try_lock().is_some());
        assert_eq!(*mutex.get_mut(), 2);
    }

    #[test]
    fn rwlock_allows_shared_reads() {
        let lock = RwLock::new(7_u32);
        {
            let first = lock.read();
            let second = lock.read();
            assert_eq!(*first + *second, 14);
        }
        *lock.write() = 9;
        assert_eq!(*lock.read(), 9);
    }

    #[test]
    fn once_runs_initializer() {
        let once = Once::new();
        let mut value = 0;
        once.call_once(|| value = 1);
        assert_eq!(value, 1);
        assert!(once.is_completed());
    }
}
