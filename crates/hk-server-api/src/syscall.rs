//! 系统调用薄封装。
//!
//! 只做参数整理与返回值解包；入口指令由 [`crate::arch`] 提供。

use hk_syscall::*;

use crate::syscall_call;

/// 非阻塞读取一个控制台字节；无输入时返回 `None`。
pub fn console_read() -> Option<u8> {
    // SAFETY: 只读内核输入缓冲，不涉及用户指针。
    let value = unsafe { syscall_call(SYS_CONSOLE_READ, 0, 0, 0, 0).value };
    (value != u64::MAX).then_some(value as u8)
}

/// 注册为控制台输入订阅者；输入到达时会收到 `CONSOLE_NOTIFY_INPUT`。
pub fn console_subscribe() -> u64 {
    // SAFETY: 只登记当前任务，不涉及用户指针。
    unsafe { syscall_call(SYS_CONSOLE_SUBSCRIBE, 0, 0, 0, 0).value }
}

/// 把文本写入内核控制台。
pub fn write(bytes: &[u8]) {
    // SAFETY: 指针与长度来自本进程的只读数据。
    unsafe {
        syscall_call(SYS_WRITE, bytes.as_ptr() as u64, bytes.len() as u64, 0, 0);
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
    unsafe { syscall_call(SYS_SEND, endpoint, label, word, 0).value }
}

/// 向端点发送消息并转移一个句柄；`transferred` 是本任务的端点编号。
pub fn send_handle(endpoint: u64, label: u64, word: u64, transferred: u64) -> u64 {
    // SAFETY: 两个端点编号均由内核分配；内核在交付时移动句柄。
    unsafe { syscall_call(SYS_SEND_HANDLE, endpoint, label, word, transferred).value }
}

/// 从端点接收消息，返回标签、消息字与发送者端点。
pub fn recv(endpoint: u64) -> (u64, u64, u64) {
    // SAFETY: 端点编号由内核分配；没有消息时内核阻塞本任务。
    let result = unsafe { syscall_call(SYS_RECV, endpoint, 0, 0, 0) };
    (result.value, result.word, result.sender)
}

/// 按服务名查询当前端点；服务不存在时返回 0。
pub fn service_endpoint(name: &str) -> u64 {
    // SAFETY: 名称来自本进程只读内存；内核只读取该范围。
    unsafe {
        syscall_call(
            SYS_SERVICE_ENDPOINT,
            name.as_ptr() as u64,
            name.len() as u64,
            0,
            0,
        )
        .value
    }
}

/// 从端点接收消息，并校验期望的发送端点。
pub fn recv_data_from(endpoint: u64, expected: u64, buffer: &mut [u8]) -> (u64, u64, u64, usize) {
    // SAFETY: 缓冲区属于本进程；内核校验范围后写入。
    let result = unsafe {
        syscall_call(
            SYS_RECV_DATA_FROM,
            endpoint,
            buffer.as_mut_ptr() as u64,
            buffer.len() as u64,
            expected,
        )
    };
    (
        result.value,
        result.word,
        result.sender,
        result.extra as usize,
    )
}

/// 从端点接收消息，并返回随消息收到的句柄编号；`0` 表示没有句柄。
pub fn recv_handle(endpoint: u64) -> (u64, u64, u64, u64) {
    // SAFETY: 端点编号由内核分配；没有消息时内核阻塞本任务。
    let result = unsafe { syscall_call(SYS_RECV, endpoint, 0, 0, 0) };
    (result.value, result.word, result.sender, result.extra)
}

/// 向端点发送带字节载荷的消息。
pub fn send_data(endpoint: u64, label: u64, data: &[u8]) -> u64 {
    // SAFETY: 数据指针与长度来自本进程的只读内存；内核复制后立即返回。
    unsafe {
        syscall_call(
            SYS_SEND_DATA,
            endpoint,
            label,
            data.as_ptr() as u64,
            data.len() as u64,
        )
        .value
    }
}

/// 从端点接收带字节载荷的消息，返回标签、消息字、发送者与载荷长度。
pub fn recv_data(endpoint: u64, buffer: &mut [u8]) -> (u64, u64, u64, usize) {
    // SAFETY: 缓冲区属于本进程；内核校验范围后写入。
    let result = unsafe {
        syscall_call(
            SYS_RECV_DATA,
            endpoint,
            buffer.as_mut_ptr() as u64,
            buffer.len() as u64,
            0,
        )
    };
    (
        result.value,
        result.word,
        result.sender,
        result.extra as usize,
    )
}

/// 非阻塞接收带字节载荷的消息；无消息时返回标签 0。
pub fn recv_data_try(endpoint: u64, buffer: &mut [u8]) -> (u64, u64, u64, usize) {
    // SAFETY: 缓冲区属于本进程；内核校验范围后写入；无消息时立即返回。
    let result = unsafe {
        syscall_call(
            SYS_RECV_DATA_TRY,
            endpoint,
            buffer.as_mut_ptr() as u64,
            buffer.len() as u64,
            0,
        )
    };
    (
        result.value,
        result.word,
        result.sender,
        result.extra as usize,
    )
}

/// 接收带字节载荷的消息，并解包随消息收到的句柄编号。
pub fn recv_data_handle(
    endpoint: u64,
    buffer: &mut [u8],
    expected: u64,
) -> (u64, u64, u64, usize, u64) {
    // SAFETY: 缓冲区属于本进程；内核校验范围后写入。
    let result = unsafe {
        syscall_call(
            SYS_RECV_DATA_HANDLE,
            endpoint,
            buffer.as_mut_ptr() as u64,
            buffer.len() as u64,
            expected,
        )
    };
    let packed = result.extra;
    (
        result.value,
        result.word,
        result.sender,
        (packed & 0xFFFF_FFFF) as usize,
        packed >> 32,
    )
}

/// 非阻塞接收带字节载荷与句柄的消息；无消息时标签为 0。
pub fn recv_data_handle_try(
    endpoint: u64,
    buffer: &mut [u8],
    expected: u64,
) -> (u64, u64, u64, usize, u64) {
    // SAFETY: 缓冲区属于本进程；内核校验范围后写入。
    let result = unsafe {
        syscall_call(
            SYS_RECV_DATA_HANDLE_TRY,
            endpoint,
            buffer.as_mut_ptr() as u64,
            buffer.len() as u64,
            expected,
        )
    };
    let packed = result.extra;
    (
        result.value,
        result.word,
        result.sender,
        (packed & 0xFFFF_FFFF) as usize,
        packed >> 32,
    )
}

/// 让出 CPU。
pub fn yield_now() -> u64 {
    // SAFETY: 让出不携带参数，内核对调用号做分派。
    unsafe { syscall_call(SYS_YIELD, 0, 0, 0, 0).value }
}

/// 置位指定任务的通知位；`target` 是端点或通知编号。
pub fn notify(target: u64, bits: u64) -> u64 {
    // SAFETY: 目标编号由内核分配；内核校验句柄后置位。
    unsafe { syscall_call(SYS_NOTIFY, target, bits, 0, 0).value }
}

/// 等待指定任务的通知位，返回命中的位。
pub fn wait_notify(target: u64, mask: u64) -> u64 {
    // SAFETY: 目标编号由内核分配；没有通知位时内核阻塞本任务。
    unsafe { syscall_call(SYS_WAIT_NOTIFY, target, mask, 0, 0).value }
}

/// 非阻塞检查通知位；没有待处理位时返回 0。
pub fn wait_notify_try(target: u64, mask: u64) -> u64 {
    // SAFETY: 目标编号由内核分配；没有通知位时立即返回。
    unsafe { syscall_call(SYS_WAIT_NOTIFY_TRY, target, mask, 0, 0).value }
}

/// 调用端点并阻塞等待回复，返回标签、消息字与服务端点。
pub fn call(endpoint: u64, label: u64, word: u64) -> (u64, u64, u64) {
    // SAFETY: 端点编号由内核分配；内核在服务回复后唤醒本任务。
    let result = unsafe { syscall_call(SYS_CALL, endpoint, label, word, 0) };
    (result.value, result.word, result.sender)
}

/// 回复最近一次收到的调用并转移一个能力句柄；成功返回 0。
pub fn reply_handle(label: u64, word: u64, handle: u64) -> u64 {
    // SAFETY: 内核校验回复令牌与句柄；句柄随回复转移给调用者。
    unsafe { syscall_call(SYS_REPLY_HANDLE, label, word, handle, 0).value }
}

/// 回复最近一次收到的调用；成功返回 0。
pub fn reply(label: u64, word: u64) -> u64 {
    // SAFETY: 内核校验回复令牌；没有待回复调用时返回失败。
    unsafe { syscall_call(SYS_REPLY, label, word, 0, 0).value }
}

/// 复制当前任务；子任务返回 0，父任务返回子任务编号。
pub fn fork() -> u64 {
    // SAFETY: 复制由内核完成；失败时返回 `u64::MAX`。
    unsafe { syscall_call(SYS_FORK, 0, 0, 0, 0).value }
}

/// 在同一地址空间创建线程，返回线程编号；失败时返回 `u64::MAX`。
pub fn thread_create(entry: u64, stack_top: u64, argument: u64, tls: u64) -> u64 {
    // SAFETY: 入口与栈都在本进程已映射的用户地址空间内。
    unsafe { syscall_call(SYS_THREAD_CREATE, entry, stack_top, argument, tls).value }
}

/// 等待线程退出；成功返回 0。
pub fn thread_join(thread: u64) -> u64 {
    // SAFETY: 线程编号来自 `thread_create`；目标存活时内核阻塞本线程。
    unsafe { syscall_call(SYS_THREAD_JOIN, thread, 0, 0, 0).value }
}

/// 注册端点接收内核投递的原始异常现场；成功返回 0。
pub fn fault_subscribe(endpoint: u64) -> u64 {
    // SAFETY: 端点编号由内核分配；内核校验句柄后登记订阅。
    unsafe { syscall_call(SYS_FAULT_SUBSCRIBE, endpoint, 0, 0, 0).value }
}

/// 设置当前线程的 TLS 基址。
pub fn thread_set_tls(tls: u64) -> u64 {
    // SAFETY: 只写当前 CPU 的线程指针寄存器。
    unsafe { syscall_call(SYS_THREAD_SET_TLS, tls, 0, 0, 0).value }
}

/// 在共享内存字上等待；值已变化时立即返回 `FUTEX_CHANGED`。
pub fn futex_wait(address: u64, expected: u32) -> u64 {
    // SAFETY: 地址来自本进程共享内存；内核只读取该字。
    unsafe { syscall_call(SYS_FUTEX_WAIT, address, u64::from(expected), 0, 0).value }
}

/// 唤醒共享内存字上的等待者，返回唤醒数量。
pub fn futex_wake(address: u64, count: u32) -> u64 {
    // SAFETY: 地址来自本进程共享内存；内核只按地址匹配等待者。
    unsafe { syscall_call(SYS_FUTEX_WAKE, address, u64::from(count), 0, 0).value }
}

/// 读取单调时间戳（TSC）。
pub fn tsc() -> u64 {
    // SAFETY: 只读计数器，不携带参数。
    unsafe { syscall_call(SYS_TSC, 0, 0, 0, 0).value }
}

/// 读取时间戳频率；未知时返回 0。
pub fn tsc_frequency() -> u64 {
    // SAFETY: 只读校准结果，不携带参数。
    unsafe { syscall_call(SYS_TSC_FREQ, 0, 0, 0, 0).value }
}

/// 读取 PCI 配置空间中的一个 32 位字。
pub fn pci_read(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    let address = (u64::from(bus) << 8) | (u64::from(device) << 3) | u64::from(function);
    // SAFETY: 内核校验参数并只访问 PCI 配置端口。
    unsafe { syscall_call(SYS_PCI_READ, address, u64::from(offset), 0, 0).value as u32 }
}

/// 写入 PCI 配置空间中的一个 32 位字。
pub fn pci_write(bus: u8, device: u8, function: u8, offset: u8, value: u32) {
    let address = (u64::from(bus) << 8) | (u64::from(device) << 3) | u64::from(function);
    // SAFETY: 内核校验参数并只访问 PCI 配置端口。
    unsafe {
        syscall_call(
            SYS_PCI_WRITE,
            address,
            u64::from(offset),
            u64::from(value),
            0,
        );
    }
}

/// 创建内核 DeviceGrant 对象，返回用户句柄。
pub fn device_grant_create(encoded: &[u8]) -> u64 {
    // SAFETY: 编码缓冲区属于本进程；内核复制后解码。
    unsafe {
        syscall_call(
            SYS_DEVICE_GRANT_CREATE,
            encoded.as_ptr() as u64,
            encoded.len() as u64,
            0,
            0,
        )
        .value
    }
}

/// 把 DeviceGrant 指定的 MMIO 范围映射到内核选择的虚拟地址。
pub fn device_map(grant: u64, index: u64) -> Option<u64> {
    let mut virt = 0_u64;
    // SAFETY: `virt` 位于本栈帧；内核校验授权范围后写入映射地址。
    let result = unsafe {
        syscall_call(
            SYS_DEVICE_MAP,
            grant,
            index,
            &mut virt as *mut u64 as u64,
            0,
        )
        .value
    };
    (result != u64::MAX && virt != 0).then_some(virt)
}

/// 把 DeviceGrant 的中断线绑定到通知位。
pub fn irq_bind(grant: u64, notification: u64, bits: u64) -> u64 {
    // SAFETY: 内核校验授权与通知句柄。
    unsafe { syscall_call(SYS_IRQ_BIND, grant, notification, bits, 0).value }
}

/// 把 MSI/MSI-X 向量绑定到通知；不建立 IOAPIC 路由。
pub fn msi_bind(grant: u64, notification: u64, bits: u64) -> u64 {
    // SAFETY: 内核校验授权与通知句柄。
    unsafe { syscall_call(SYS_MSI_BIND, grant, notification, bits, 0).value }
}

/// 同步 DeviceGrant 的 DMA 缓冲缓存；`to_device` 为真时写回，否则使失效。
pub fn dma_sync(grant: u64, phys: u64, len: usize, to_device: bool) -> u64 {
    // SAFETY: 内核校验物理范围属于该授权。
    unsafe { syscall_call(SYS_DMA_SYNC, grant, phys, len as u64, u64::from(to_device)).value }
}

/// 读取 DeviceGrant 的固定布局编码。
pub fn device_info(grant: u64, buffer: &mut [u8]) -> u64 {
    // SAFETY: 内核校验句柄并只写入给定用户缓冲。
    unsafe {
        syscall_call(
            SYS_DEVICE_INFO,
            grant,
            buffer.len() as u64,
            buffer.as_mut_ptr() as u64,
            0,
        )
        .value
    }
}

/// 返回当前任务自己的通知编号。
pub fn self_notification() -> u64 {
    // SAFETY: 只读取运行时任务槽位，不涉及指针。
    unsafe { syscall_call(SYS_SELF_NOTIFICATION, 0, 0, 0, 0).value }
}

/// 复制句柄并收窄权限，返回新句柄编号。
pub fn handle_duplicate(number: u64, rights: u32) -> u64 {
    // SAFETY: 内核校验句柄与权限后复制。
    unsafe { syscall_call(SYS_HANDLE_DUPLICATE, number, u64::from(rights), 0, 0).value }
}

/// 读取平台启动命令行；未提供时返回 `None`。
pub fn boot_cmdline(buffer: &mut [u8]) -> Option<&str> {
    // SAFETY: 缓冲区属于本进程；内核校验长度后写入。
    let result = unsafe {
        syscall_call(
            SYS_BOOT_CMDLINE,
            buffer.as_mut_ptr() as u64,
            buffer.len() as u64,
            0,
            0,
        )
    };
    let length = result.value as usize;
    if length == 0 || length > buffer.len() {
        return None;
    }
    core::str::from_utf8(&buffer[..length]).ok()
}

/// 读取民用时间；没有 RTC 时返回 `None`。
pub fn civil_time() -> Option<hk_base::CivilTime> {
    let mut bytes = [0_u8; hk_base::CivilTime::ENCODED_LEN];
    // SAFETY: 缓冲区属于本进程；内核校验长度后写入。
    let result = unsafe {
        syscall_call(
            SYS_CIVIL_TIME,
            bytes.as_mut_ptr() as u64,
            bytes.len() as u64,
            0,
            0,
        )
    };
    if result.value as usize != hk_base::CivilTime::ENCODED_LEN {
        return None;
    }
    hk_base::CivilTime::decode(&bytes)
}

/// 读取调度 tick 计数。
pub fn ticks() -> u64 {
    // SAFETY: 只读计数器，不携带参数。
    unsafe { syscall_call(SYS_TICKS, 0, 0, 0, 0).value }
}

/// 在任务 Arena 内分配并清零内存；失败时返回空指针。
pub fn alloc(size: usize, align: usize) -> *mut u8 {
    // SAFETY: 内核在 Arena 内按对齐分配并清零，返回用户可写地址。
    unsafe { syscall_call(SYS_ALLOC, size as u64, align as u64, 0, 0).value as *mut u8 }
}

/// 关闭端点句柄；成功返回 0，失败返回 `u64::MAX`。
pub fn close_handle(endpoint: u64) -> u64 {
    // SAFETY: 端点编号由内核分配；内核校验句柄后关闭。
    unsafe { syscall_call(SYS_HANDLE_CLOSE, endpoint, 0, 0, 0).value }
}

/// 返回当前任务的在用句柄数。
pub fn handle_count() -> u64 {
    // SAFETY: 只读计数，不携带参数。
    unsafe { syscall_call(SYS_HANDLE_COUNT, 0, 0, 0, 0).value }
}

/// 内核运行统计。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KernelStats {
    /// 内核线程数。
    pub threads: u16,
    /// 存活内核对象数。
    pub objects: u16,
    /// 就绪线程数。
    pub runnable: u16,
    /// trace 事件数。
    pub trace_events: u16,
}

/// 读取内核运行统计。
pub fn kernel_stats() -> KernelStats {
    // SAFETY: 只读计数，不携带指针或可变状态。
    let packed = unsafe { syscall_call(SYS_KERNEL_STATS, 0, 0, 0, 0).value };
    KernelStats {
        threads: (packed >> 48) as u16,
        objects: (packed >> 32) as u16,
        runnable: (packed >> 16) as u16,
        trace_events: packed as u16,
    }
}

/// 结束服务，不返回。
pub fn exit(code: u64) -> ! {
    // SAFETY: 退出调用不返回；内核据此标记任务退出并切换调度。
    unsafe {
        syscall_call(SYS_EXIT, code, 0, 0, 0);
    }
    loop {
        core::hint::spin_loop();
    }
}
