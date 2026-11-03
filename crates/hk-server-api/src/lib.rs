//! 内核与服务器之间的系统调用 ABI。
//!
//! 服务在用户态通过 `int 0x80` 进入内核：`rax` 为调用号，`rdi`/`rsi`/`rdx` 为参数。

#![no_std]

use core::arch::asm;

/// 退出服务。
pub const SYS_EXIT: u64 = 0;

/// 把一段 UTF-8 文本写入内核控制台。
pub const SYS_WRITE: u64 = 1;

/// 让出 CPU，调度器切换到下一个可运行任务。
pub const SYS_YIELD: u64 = 2;

/// 向端点发送消息：`rdi` 为端点编号，`rsi` 为标签，`rdx` 为消息字。
pub const SYS_SEND: u64 = 3;

/// 从端点接收消息：`rdi` 为端点编号。
///
/// 返回 `rax` 为标签、`rdx` 为消息字、`rcx` 为发送者端点；没有消息时任务阻塞。
pub const SYS_RECV: u64 = 4;

/// 读取 PCI 配置空间：`rdi` 为 bus/device/function 编码，`rsi` 为偏移。
pub const SYS_PCI_READ: u64 = 5;

/// 读取调度 tick 计数。
pub const SYS_TICKS: u64 = 6;

/// 在任务 Arena 内分配内存。
pub const SYS_ALLOC: u64 = 7;

/// 关闭当前任务的端点句柄。
pub const SYS_HANDLE_CLOSE: u64 = 8;

/// 查询当前任务的在用句柄数。
pub const SYS_HANDLE_COUNT: u64 = 9;

/// echo 服务的端点编号。
pub const ENDPOINT_ECHO: u64 = 2;

/// 发起系统调用。
///
/// # Safety
/// 调用方必须处于用户态或由内核显式调用；参数语义由调用号决定。
#[inline(always)]
unsafe fn syscall(number: u64, first: u64, second: u64) -> u64 {
    let result: u64;
    // SAFETY: `int 0x80` 由内核在 IDT 中注册；寄存器约定与内核分派一致。
    unsafe {
        asm!(
            "int 0x80",
            inlateout("rax") number => result,
            in("rdi") first,
            in("rsi") second,
            options(nostack),
        );
    }
    result
}

/// 发起带三个参数的系统调用，返回 `rax`、`rdx` 与 `rcx`。
///
/// # Safety
/// 调用方必须处于用户态或由内核显式调用；参数语义由调用号决定。
#[inline(always)]
unsafe fn syscall4(number: u64, first: u64, second: u64, third: u64) -> (u64, u64, u64) {
    let result: u64;
    let word: u64;
    let sender: u64;
    // SAFETY: `int 0x80` 由内核在 IDT 中注册；内核恢复 `rax`、`rdx` 与 `rcx`。
    unsafe {
        asm!(
            "int 0x80",
            inlateout("rax") number => result,
            in("rdi") first,
            in("rsi") second,
            inlateout("rdx") third => word,
            lateout("rcx") sender,
            options(nostack),
        );
    }
    (result, word, sender)
}

/// 把文本写入内核控制台。
pub fn write(bytes: &[u8]) {
    // SAFETY: 指针与长度来自本进程的只读数据。
    unsafe {
        syscall(SYS_WRITE, bytes.as_ptr() as u64, bytes.len() as u64);
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
    unsafe { syscall4(SYS_SEND, endpoint, label, word).0 }
}

/// 从端点接收消息，返回标签、消息字与发送者端点。
pub fn recv(endpoint: u64) -> (u64, u64, u64) {
    // SAFETY: 端点编号由内核分配；没有消息时内核阻塞本任务。
    unsafe { syscall4(SYS_RECV, endpoint, 0, 0) }
}

/// 让出 CPU。
pub fn yield_now() -> u64 {
    // SAFETY: 让出不携带参数，内核对调用号做分派。
    unsafe { syscall(SYS_YIELD, 0, 0) }
}

/// 读取 PCI 配置空间中的一个 32 位字。
pub fn pci_read(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    let address = (u64::from(bus) << 8) | (u64::from(device) << 3) | u64::from(function);
    // SAFETY: 内核校验参数并只访问 PCI 配置端口。
    unsafe { syscall(SYS_PCI_READ, address, u64::from(offset)) as u32 }
}

/// 读取调度 tick 计数。
pub fn ticks() -> u64 {
    // SAFETY: 只读计数器，不携带参数。
    unsafe { syscall(SYS_TICKS, 0, 0) }
}

/// 在任务 Arena 内分配并清零内存；失败时返回空指针。
pub fn alloc(size: usize, align: usize) -> *mut u8 {
    // SAFETY: 内核在 Arena 内按对齐分配并清零，返回用户可写地址。
    unsafe { syscall(SYS_ALLOC, size as u64, align as u64) as *mut u8 }
}

/// 关闭端点句柄；成功返回 0，失败返回 `u64::MAX`。
pub fn close_handle(endpoint: u64) -> u64 {
    // SAFETY: 端点编号由内核分配；内核校验句柄后关闭。
    unsafe { syscall(SYS_HANDLE_CLOSE, endpoint, 0) }
}

/// 返回当前任务的在用句柄数。
pub fn handle_count() -> u64 {
    // SAFETY: 只读计数，不携带参数。
    unsafe { syscall(SYS_HANDLE_COUNT, 0, 0) }
}

/// 结束服务，不返回。
pub fn exit(code: u64) -> ! {
    // SAFETY: 退出调用不返回；内核据此标记任务退出并切换调度。
    unsafe {
        syscall(SYS_EXIT, code, 0);
    }
    loop {
        core::hint::spin_loop();
    }
}
