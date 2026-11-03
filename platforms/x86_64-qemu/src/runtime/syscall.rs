//! 系统调用分派与实现。
//!
//! 每个调用把参数从 [`InterruptFrame`] 取出，更新任务现场或内核状态，并返回
//! [`SyscallAction`] 告诉上层是继续用户态还是切换任务。

use hk_ipc::Message;
use hk_kernel::{TcbId, ThreadState};
use hk_platform_x86_64::cpu::{self, InterruptFrame, SyscallAction};

use super::{Pending, RUNTIME, Runtime, endpoint_of};

pub(super) fn dispatch(frame: &mut InterruptFrame) -> SyscallAction {
    let mut guard = RUNTIME.lock();
    let Some(runtime) = guard.as_mut() else {
        frame.rax = u64::MAX;
        return SyscallAction::Continue;
    };
    let Some(index) = runtime.current else {
        frame.rax = u64::MAX;
        return SyscallAction::Continue;
    };

    let tcb = runtime.tasks[index].tcb;
    match frame.rax {
        hk_server_api::SYS_SEND => send(runtime, index, tcb, frame),
        hk_server_api::SYS_RECV => recv(runtime, index, tcb, frame),
        hk_server_api::SYS_YIELD => {
            runtime.tasks[index].context = *frame;
            runtime.tasks[index].pending = Pending::None;
            SyscallAction::Block
        }
        hk_server_api::SYS_EXIT => {
            let code = frame.rdi;
            let _ = runtime.kernel.exit(tcb);
            runtime.tasks[index].exited = true;
            runtime.tasks[index].exit_code = code;
            SyscallAction::Exit
        }
        hk_server_api::SYS_PCI_READ => {
            let address = frame.rdi;
            let bus = (address >> 8) as u8;
            let device = ((address >> 3) & 0x1F) as u8;
            let function = (address & 0x07) as u8;
            frame.rax = u64::from(hk_platform_x86_64::pci::config_read(
                bus,
                device,
                function,
                frame.rsi as u8,
            ));
            SyscallAction::Continue
        }
        hk_server_api::SYS_TICKS => {
            frame.rax = cpu::timer_ticks();
            SyscallAction::Continue
        }
        hk_server_api::SYS_ALLOC => {
            frame.rax =
                allocate(runtime, index, frame.rdi as usize, frame.rsi as usize).unwrap_or(0);
            SyscallAction::Continue
        }
        hk_server_api::SYS_HANDLE_COUNT => {
            frame.rax = runtime.kernel.handles_live(tcb).unwrap_or(0) as u64;
            SyscallAction::Continue
        }
        hk_server_api::SYS_HANDLE_CLOSE => {
            frame.rax = close_handle(runtime, index, tcb, frame.rdi as usize);
            SyscallAction::Continue
        }
        _ => {
            frame.rax = u64::MAX;
            SyscallAction::Continue
        }
    }
}

fn send(
    runtime: &mut Runtime,
    index: usize,
    tcb: TcbId,
    frame: &mut InterruptFrame,
) -> SyscallAction {
    let endpoint = frame.rdi as usize;
    let Some(handle) = runtime.tasks[index]
        .handles
        .get(endpoint.wrapping_sub(1))
        .copied()
        .flatten()
    else {
        frame.rax = u64::MAX;
        return SyscallAction::Continue;
    };

    let mut words = [0; 8];
    words[0] = frame.rdx;
    let message = Message::new(frame.rsi).with_words(words);

    match runtime.kernel.send(tcb, handle, message) {
        Ok(()) => {
            if runtime.kernel.state_of(tcb) == Some(ThreadState::Blocked) {
                let mut context = *frame;
                context.rax = 0;
                runtime.tasks[index].context = context;
                runtime.tasks[index].pending = Pending::Send;
                SyscallAction::Block
            } else {
                frame.rax = 0;
                SyscallAction::Continue
            }
        }
        Err(_) => {
            frame.rax = u64::MAX;
            SyscallAction::Continue
        }
    }
}

fn recv(
    runtime: &mut Runtime,
    index: usize,
    tcb: TcbId,
    frame: &mut InterruptFrame,
) -> SyscallAction {
    let endpoint = frame.rdi as usize;
    let Some(handle) = runtime.tasks[index]
        .handles
        .get(endpoint.wrapping_sub(1))
        .copied()
        .flatten()
    else {
        frame.rax = u64::MAX;
        return SyscallAction::Continue;
    };

    match runtime.kernel.recv(tcb, handle) {
        Ok(Some(received)) => {
            let sender = endpoint_of(runtime, received.sender);
            frame.rax = received.message.label;
            frame.rdx = received.message.words[0];
            frame.rcx = sender;
            SyscallAction::Continue
        }
        Ok(None) => {
            let mut context = *frame;
            context.rax = 0;
            runtime.tasks[index].context = context;
            runtime.tasks[index].pending = Pending::Recv { handle };
            SyscallAction::Block
        }
        Err(_) => {
            frame.rax = u64::MAX;
            SyscallAction::Continue
        }
    }
}

fn allocate(runtime: &mut Runtime, index: usize, size: usize, align: usize) -> Option<u64> {
    if size == 0 || !align.is_power_of_two() {
        return None;
    }

    let align = align.max(8) as u64;
    let task = &mut runtime.tasks[index];
    let start = (task.arena_next + align - 1) & !(align - 1);
    let end = start.checked_add(size as u64)?;
    if end > task.arena_end {
        return None;
    }

    // SAFETY: Arena 页已映射为用户可写；内核与用户共享同一页表，可按用户地址清零。
    unsafe {
        core::ptr::write_bytes(start as *mut u8, 0, size);
    }
    task.arena_next = end;
    Some(start)
}

fn close_handle(runtime: &mut Runtime, index: usize, tcb: TcbId, endpoint: usize) -> u64 {
    let slot = endpoint.wrapping_sub(1);
    let Some(handle) = runtime.tasks[index].handles.get(slot).copied().flatten() else {
        return u64::MAX;
    };

    match runtime.kernel.close_handle(tcb, handle) {
        Ok(()) => {
            runtime.tasks[index].handles[slot] = None;
            0
        }
        Err(_) => u64::MAX,
    }
}
