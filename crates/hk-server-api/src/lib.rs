//! 内核与服务器之间的系统调用 ABI。
//!
//! 本 crate 只描述调用号、参数与返回约定；具体入口指令由 [`arch`] 按架构提供。

#![no_std]

mod arch;

pub use hk_syscall::*;

use arch::call;

/// 把文本写入内核控制台。
pub fn write(bytes: &[u8]) {
    // SAFETY: 指针与长度来自本进程的只读数据。
    unsafe {
        call(SYS_WRITE, bytes.as_ptr() as u64, bytes.len() as u64, 0, 0);
    }
}

/// 以十进制输出一个无符号整数。
pub fn write_u64(mut value: u64) {
    if value == 0 {
        write(b"0");
        return;
    }

    let mut buffer = [0_u8; 20];
    let mut index = buffer.len();
    while value > 0 {
        index -= 1;
        buffer[index] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    write(&buffer[index..]);
}

/// 向端点发送消息。
pub fn send(endpoint: u64, label: u64, word: u64) -> u64 {
    // SAFETY: 端点编号由内核分配；消息字按值传递。
    unsafe { call(SYS_SEND, endpoint, label, word, 0).value }
}

/// 向端点发送消息并转移一个句柄；`transferred` 是本任务的端点编号。
pub fn send_handle(endpoint: u64, label: u64, word: u64, transferred: u64) -> u64 {
    // SAFETY: 两个端点编号均由内核分配；内核在交付时移动句柄。
    unsafe { call(SYS_SEND_HANDLE, endpoint, label, word, transferred).value }
}

/// 从端点接收消息，返回标签、消息字与发送者端点。
pub fn recv(endpoint: u64) -> (u64, u64, u64) {
    // SAFETY: 端点编号由内核分配；没有消息时内核阻塞本任务。
    let result = unsafe { call(SYS_RECV, endpoint, 0, 0, 0) };
    (result.value, result.word, result.sender)
}

/// 从端点接收消息，并返回随消息收到的句柄编号；`0` 表示没有句柄。
pub fn recv_handle(endpoint: u64) -> (u64, u64, u64, u64) {
    // SAFETY: 端点编号由内核分配；没有消息时内核阻塞本任务。
    let result = unsafe { call(SYS_RECV, endpoint, 0, 0, 0) };
    (result.value, result.word, result.sender, result.extra)
}

/// 让出 CPU。
pub fn yield_now() -> u64 {
    // SAFETY: 让出不携带参数，内核对调用号做分派。
    unsafe { call(SYS_YIELD, 0, 0, 0, 0).value }
}

/// 读取 PCI 配置空间中的一个 32 位字。
pub fn pci_read(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    let address = (u64::from(bus) << 8) | (u64::from(device) << 3) | u64::from(function);
    // SAFETY: 内核校验参数并只访问 PCI 配置端口。
    unsafe { call(SYS_PCI_READ, address, u64::from(offset), 0, 0).value as u32 }
}

/// 读取调度 tick 计数。
pub fn ticks() -> u64 {
    // SAFETY: 只读计数器，不携带参数。
    unsafe { call(SYS_TICKS, 0, 0, 0, 0).value }
}

/// 在任务 Arena 内分配并清零内存；失败时返回空指针。
pub fn alloc(size: usize, align: usize) -> *mut u8 {
    // SAFETY: 内核在 Arena 内按对齐分配并清零，返回用户可写地址。
    unsafe { call(SYS_ALLOC, size as u64, align as u64, 0, 0).value as *mut u8 }
}

/// 关闭端点句柄；成功返回 0，失败返回 `u64::MAX`。
pub fn close_handle(endpoint: u64) -> u64 {
    // SAFETY: 端点编号由内核分配；内核校验句柄后关闭。
    unsafe { call(SYS_HANDLE_CLOSE, endpoint, 0, 0, 0).value }
}

/// 返回当前任务的在用句柄数。
pub fn handle_count() -> u64 {
    // SAFETY: 只读计数，不携带参数。
    unsafe { call(SYS_HANDLE_COUNT, 0, 0, 0, 0).value }
}

/// 结束服务，不返回。
pub fn exit(code: u64) -> ! {
    // SAFETY: 退出调用不返回；内核据此标记任务退出并切换调度。
    unsafe {
        call(SYS_EXIT, code, 0, 0, 0);
    }
    loop {
        core::hint::spin_loop();
    }
}
