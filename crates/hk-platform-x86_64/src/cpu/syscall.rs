//! 系统调用入口与用户内存访问。

use core::sync::atomic::{AtomicUsize, Ordering};

use hk_hal::Console;

use super::user::leave_user;
use super::*;
use crate::serial::{COM1, Serial};

/// 系统调用处理结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyscallAction {
    /// 已就地处理，返回用户态。
    Continue,
    /// 任务阻塞，调度器切换到下一个任务。
    Block,
    /// 任务退出，调度器切换到下一个任务。
    Exit,
}

/// 扩展系统调用处理函数。
pub type SyscallHook = fn(&mut InterruptFrame) -> SyscallAction;

/// 用户态抢占钩子：保存现场并返回是否切换任务。
pub type PreemptHook = fn(&InterruptFrame) -> bool;
static SYSCALL_HOOK: AtomicUsize = AtomicUsize::new(0);
static PREEMPT_HOOK: AtomicUsize = AtomicUsize::new(0);
/// 用户态可访问的地址上限。
const USER_LIMIT: u64 = 0x0000_0080_0000_0000;

/// 系统调用允许的最大写入长度。
const MAX_WRITE: usize = 4096;

pub(super) fn handle_syscall(frame: &mut InterruptFrame) {
    if frame.rax == hk_syscall::SYS_WRITE {
        let pointer = frame.rdi;
        let length = frame.rsi as usize;
        frame.rax = if user_range_valid(pointer, length) {
            write_user(pointer, length)
        } else {
            u64::MAX
        };
        return;
    }

    let hook = SYSCALL_HOOK.load(Ordering::SeqCst);
    if hook != 0 {
        // SAFETY: 钩子由内核在进入用户态之前注册，指向静态函数并覆盖整个内核运行期。
        let hook: SyscallHook = unsafe { core::mem::transmute(hook) };
        match hook(frame) {
            SyscallAction::Continue => {}
            SyscallAction::Block | SyscallAction::Exit => leave_user(),
        }
        return;
    }

    match frame.rax {
        hk_syscall::SYS_EXIT => leave_user(),
        _ => frame.rax = u64::MAX,
    }
}

/// 注册扩展系统调用处理函数。
pub fn set_syscall_hook(hook: SyscallHook) {
    SYSCALL_HOOK.store(hook as usize, Ordering::SeqCst);
}

/// 注册用户态抢占钩子。
pub fn set_preempt_hook(hook: PreemptHook) {
    PREEMPT_HOOK.store(hook as usize, Ordering::SeqCst);
}

pub(super) fn preempt(frame: &InterruptFrame) -> bool {
    let hook = PREEMPT_HOOK.load(Ordering::SeqCst);
    if hook == 0 {
        return false;
    }
    // SAFETY: 钩子在进入用户态前注册，指向静态函数并覆盖整个内核运行期。
    let hook: PreemptHook = unsafe { core::mem::transmute(hook) };
    hook(frame)
}

fn user_range_valid(pointer: u64, length: usize) -> bool {
    length <= MAX_WRITE
        && pointer >= 0x1000
        && pointer
            .checked_add(length as u64)
            .is_some_and(|end| end <= USER_LIMIT)
}

fn write_user(pointer: u64, length: usize) -> u64 {
    // SAFETY: 调用前已校验地址落在用户可访问范围内，且进程页表中有映射。
    let bytes = unsafe { core::slice::from_raw_parts(pointer as *const u8, length) };
    match core::str::from_utf8(bytes) {
        Ok(text) => {
            Serial::new(COM1).write_str(text);
            length as u64
        }
        Err(_) => u64::MAX,
    }
}
