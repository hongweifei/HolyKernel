//! 内核与服务器共享的系统调用 ABI。
//!
//! 系统调用号按子系统分段，每段预留扩展空间；编号发布后保持稳定，新调用在
//! 所属段内顺序追加。
//!
//! | 区间 | 子系统 |
//! | --- | --- |
//! | `0x00-0x0F` | 进程与线程 |
//! | `0x10-0x1F` | 句柄 |
//! | `0x20-0x3F` | IPC、通知与同步 |
//! | `0x40-0x4F` | 内存与共享 Frame |
//! | `0x50-0x5F` | 设备与中断 |
//! | `0x60-0x6F` | 时间与系统信息 |
//! | `0x70-0x7F` | 控制台与帧缓冲 |
//! | `0x80-0x8F` | Root 与调试控制 |
//! | `0x90-0x9F` | HMP 对象查询 |
//!
//! 只包含系统调用编号、跨边界共享常量与编码，不依赖任何 crate 或平台；寄存器
//! 映射由平台层实现，服务协议与服务策略放在各自的用户态库。

#![no_std]

// === 进程与线程 0x00-0x0F ===

/// 退出服务。
pub const SYS_EXIT: u64 = 0x00;

/// 让出 CPU。
pub const SYS_YIELD: u64 = 0x01;

/// 复制当前任务。
pub const SYS_FORK: u64 = 0x02;

/// 在同一地址空间创建线程。
pub const SYS_THREAD_CREATE: u64 = 0x03;

/// 等待线程退出。
pub const SYS_THREAD_JOIN: u64 = 0x04;

/// 设置当前线程的 TLS 基址。
pub const SYS_THREAD_SET_TLS: u64 = 0x05;

/// 休眠指定调度 tick 数；到期后由内核唤醒。
pub const SYS_SLEEP: u64 = 0x06;

/// 一个调度 tick 对应的契约微秒数（10 ms）。
///
/// `SYS_SLEEP` 的参数以该 tick 为单位；Cadence 的预算、周期与截止期则使用
/// 1 µs 契约时基，两者在 ABI 边界显式区分。
pub const SCHED_TICK_US: u64 = 10_000;

/// 查询 Cadence 契约；`handle = 0` 表示当前 Task。
pub const SYS_CADENCE_INFO: u64 = 0x07;

/// 在父契约范围内调整 Cadence。
pub const SYS_CADENCE_UPDATE: u64 = 0x08;

/// 派生一个子 Cadence 契约。
pub const SYS_CADENCE_SPLIT: u64 = 0x09;

/// 撤销 Cadence 契约；递增 epoch 使旧句柄失效。
pub const SYS_CADENCE_REVOKE: u64 = 0x0A;

/// 查询 Meter 计量；`handle = 0` 表示当前 Task。
pub const SYS_METER_INFO: u64 = 0x0B;

/// 为 Task 附加 Cadence 契约；`task = 0` 表示当前 Task。
pub const SYS_TASK_ATTACH_CADENCE: u64 = 0x0C;

/// 取得当前 Task 的 Cadence 句柄；持有策略授权的 Task 同时获得 ADMIN。
pub const SYS_CADENCE_SELF: u64 = 0x0D;

/// 按任务名附加 Cadence；仅策略授权任务可用。
pub const SYS_TASK_ATTACH_NAMED: u64 = 0x0E;

/// 按任务名撤销旧 epoch 并附加新 Cadence；仅策略授权任务可用。
pub const SYS_TASK_REGRANT_NAMED: u64 = 0x0F;

// === 句柄 0x10-0x1F ===

/// 关闭端点句柄。
pub const SYS_HANDLE_CLOSE: u64 = 0x10;

/// 查询在用句柄数。
pub const SYS_HANDLE_COUNT: u64 = 0x11;

/// 复制句柄。
pub const SYS_HANDLE_DUPLICATE: u64 = 0x12;

// === IPC、通知与同步 0x20-0x3F ===

/// 向端点发送消息。
pub const SYS_SEND: u64 = 0x20;

/// 发送消息并转移一个句柄。
pub const SYS_SEND_HANDLE: u64 = 0x21;

/// 发送带字节载荷的消息。
pub const SYS_SEND_DATA: u64 = 0x22;

/// 从端点接收消息。
pub const SYS_RECV: u64 = 0x23;

/// 接收带字节载荷的消息。
pub const SYS_RECV_DATA: u64 = 0x24;

/// 接收带字节载荷的消息，并指定期望的发送端点。
pub const SYS_RECV_DATA_FROM: u64 = 0x25;

/// 接收带字节载荷的消息，并把随消息收到的句柄编号打包返回。
pub const SYS_RECV_DATA_HANDLE: u64 = 0x26;

/// 非阻塞接收带字节载荷的消息；无消息时返回 0。
pub const SYS_RECV_DATA_TRY: u64 = 0x27;

/// 非阻塞接收带字节载荷与句柄的消息；无消息时返回 0。
pub const SYS_RECV_DATA_HANDLE_TRY: u64 = 0x28;

/// 调用端点并等待回复。
pub const SYS_CALL: u64 = 0x29;

/// 回复最近一次调用。
pub const SYS_REPLY: u64 = 0x2A;

/// 回复最近一次调用并转移一个句柄。
pub const SYS_REPLY_HANDLE: u64 = 0x32;

/// 置位通知位。
pub const SYS_NOTIFY: u64 = 0x2B;

/// 等待通知位。
pub const SYS_WAIT_NOTIFY: u64 = 0x2C;

/// 非阻塞检查通知位；没有待处理位时返回 0。
pub const SYS_WAIT_NOTIFY_TRY: u64 = 0x2D;

/// 返回当前任务自己的通知编号。
pub const SYS_SELF_NOTIFICATION: u64 = 0x2E;

/// 按服务名查询当前任务端点。
pub const SYS_SERVICE_ENDPOINT: u64 = 0x2F;

// === 同步 0x30-0x3F ===

/// 在共享内存字上等待。
pub const SYS_FUTEX_WAIT: u64 = 0x30;

/// 唤醒共享内存字上的等待者。
pub const SYS_FUTEX_WAKE: u64 = 0x31;

/// 带超时接收带字节载荷的消息；超时返回 `u64::MAX`。
pub const SYS_RECV_DATA_TIMEOUT: u64 = 0x33;

/// 带超时调用端点；超时返回 `u64::MAX`。
pub const SYS_CALL_TIMEOUT: u64 = 0x34;

/// 按任务名读取 Cadence 快照；仅策略授权任务可用。
pub const SYS_CADENCE_NAMED: u64 = 0x35;

/// `SYS_FUTEX_WAIT`：值已变化，无需等待。
pub const FUTEX_CHANGED: u64 = 1;

// === 内存与共享 Frame 0x40-0x4F ===

/// 在任务 Arena 内分配内存。
pub const SYS_ALLOC: u64 = 0x40;

/// 类型化能力的 ABI 编码；内核与用户态共用同一组判别值。
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityKind {
    /// 文档修订。
    Revision = 0,
    /// 显示表面。
    Surface = 1,
    /// 原生任务镜像区域。
    TaskImage = 2,
    /// 显存/DMA 分配。
    DeviceAllocation = 3,
    /// 匿名内存。
    Anonymous = 4,
    /// 物理页。
    Frame = 5,
}

impl CapabilityKind {
    /// 把 ABI 编码还原为能力类型；未知编码返回 `None`。
    pub const fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            0 => Self::Revision,
            1 => Self::Surface,
            2 => Self::TaskImage,
            3 => Self::DeviceAllocation,
            4 => Self::Anonymous,
            5 => Self::Frame,
            _ => return None,
        })
    }

    /// 返回 ABI 编码。
    pub const fn as_u8(self) -> u8 {
        self as u8
    }
}

/// 创建类型化能力并分配常驻物理页。
pub const SYS_CAPABILITY_CREATE: u64 = 0x41;

/// 把类型化能力的一段映射进调用者 VSpace。
pub const SYS_REGION_MAP: u64 = 0x42;

/// 查询 Region 的虚拟地址与长度。
pub const SYS_REGION_INFO: u64 = 0x43;

/// 解除 Region 映射。
pub const SYS_REGION_UNMAP: u64 = 0x44;

/// 查询类型化能力的物理地址与长度。
pub const SYS_CAPABILITY_PHYS: u64 = 0x45;

/// 修改已映射 Region 的权利与标志。
pub const SYS_REGION_PROTECT: u64 = 0x46;

/// 把类型化能力绑定到 DeviceGrant 的 IOMMU 域。
pub const SYS_DMA_MAP_CAPABILITY: u64 = 0x47;

/// 同步 DMA 缓冲缓存。
pub const SYS_DMA_SYNC: u64 = 0x48;

/// 固定或解除固定类型化能力。
pub const SYS_CAPABILITY_PIN: u64 = 0x49;

/// 设置 Surface 的 scanout 驻留状态。
pub const SYS_CAPABILITY_SCANOUT: u64 = 0x4A;

/// 提交 Revision 能力，推进版本。
pub const SYS_CAPABILITY_COMMIT: u64 = 0x4B;

/// 导出可转移的类型化能力句柄。
pub const SYS_CAPABILITY_EXPORT: u64 = 0x4C;

/// 把类型化能力绑定到 Provider 端点。
pub const SYS_CAPABILITY_BIND_PROVIDER: u64 = 0x4D;

/// 创建按需装入的匿名能力。
pub const SYS_CAPABILITY_CREATE_LAZY: u64 = 0x4E;

/// 查询任务 Ledger 的用量、配额、峰值与压力。
pub const SYS_LEDGER_INFO: u64 = 0x4F;

// === HMP 对象查询 0x90-0x9F ===

/// 查询当前任务 Arena 的基址、大小、已用与 Region 数。
pub const SYS_ARENA_INFO: u64 = 0x90;

/// 查询 Capacity 的区间、用途、页大小、配额与用量。
pub const SYS_CAPACITY_INFO: u64 = 0x91;

/// 从 Capacity 尾部切出子 Capacity。
pub const SYS_CAPACITY_SPLIT: u64 = 0x92;

/// 从 Capacity 分配一个 Frame。
pub const SYS_CAPACITY_ALLOC: u64 = 0x93;

/// 把 Frame 归还给 Capacity。
pub const SYS_CAPACITY_FREE: u64 = 0x94;

/// 查询 Frame 的物理区间、页大小、所属 Capacity、引用数与固定状态。
pub const SYS_FRAME_INFO: u64 = 0x95;

/// 固定或解除固定 Frame。
pub const SYS_FRAME_PIN: u64 = 0x96;

/// 增加 Frame 共享引用。
pub const SYS_FRAME_SHARE: u64 = 0x97;

/// 释放一次 Frame 共享引用。
pub const SYS_FRAME_RELEASE: u64 = 0x98;

/// 把 Frame 归还给 Capacity。
pub const SYS_FRAME_FREE: u64 = 0x99;

/// 把 Frame 映射进当前任务地址空间。
pub const SYS_FRAME_MAP: u64 = 0x9A;

/// 把 Frame 绑定到 DeviceGrant 的 IOMMU 域。
pub const SYS_DMA_MAP_FRAME: u64 = 0x9B;

/// 查询 System Capacity 的 FrameBank 记账快照。
pub const SYS_MEMORY_STATS: u64 = 0x9C;

/// 把一个已清空的子 Capacity 并回父 Capacity。
pub const SYS_CAPACITY_MERGE: u64 = 0x9D;

/// 按索引枚举容量 Ledger 快照。
pub const SYS_LEDGER_ENUM: u64 = 0x9E;

/// 按 `GROW` 标志扩展 Region；参数为 Region 句柄与新总长度。
pub const SYS_REGION_GROW: u64 = 0x9F;

/// 容量 Ledger 单条记录字节数。
pub const LEDGER_RECORD_BYTES: usize = 64;

/// 初始 Capacity 句柄编号；Root Server 授予每个内存服务一个起始容量。
pub const INITIAL_CAPACITY_HANDLE: u64 = 0x5001;

/// Capacity 用途编码：System。
pub const CAPACITY_KIND_SYSTEM: u8 = 0;
/// Capacity 用途编码：User。
pub const CAPACITY_KIND_USER: u8 = 1;
/// Capacity 用途编码：Dma。
pub const CAPACITY_KIND_DMA: u8 = 2;
/// Capacity 用途编码：Gpu。
pub const CAPACITY_KIND_GPU: u8 = 3;
/// Capacity 用途编码：Numa。
pub const CAPACITY_KIND_NUMA: u8 = 4;

// === 设备与中断 0x50-0x5F ===

/// 读取 PCI 配置空间。
pub const SYS_PCI_READ: u64 = 0x50;

/// 写入 PCI 配置空间。
pub const SYS_PCI_WRITE: u64 = 0x51;

/// 创建 DeviceGrant 对象。
pub const SYS_DEVICE_GRANT_CREATE: u64 = 0x52;

/// 把 DeviceGrant 的 MMIO 范围映射到当前任务。
pub const SYS_DEVICE_MAP: u64 = 0x53;

/// 查询 DeviceGrant 编码描述。
pub const SYS_DEVICE_INFO: u64 = 0x54;

/// 把设备中断绑定到通知。
pub const SYS_IRQ_BIND: u64 = 0x55;

/// 把 MSI/MSI-X 向量绑定到通知；只记录投递目标，不建立 IOAPIC 路由。
pub const SYS_MSI_BIND: u64 = 0x56;

/// 订阅 ACPI SCI/GPE 事件通知。
pub const SYS_ACPI_LISTEN: u64 = 0x57;

/// 取出并清除待处理的 ACPI GPE 事件位图。
pub const SYS_ACPI_EVENTS: u64 = 0x58;

/// 读取当前 CPU 温度（摄氏度）；不支持时返回 `u64::MAX`。
pub const SYS_THERMAL: u64 = 0x59;

/// 读取逻辑 CPU 的 ACPI/SRAT/PPTT 拓扑。
pub const SYS_CPU_TOPOLOGY: u64 = 0x5A;

// === 时间与系统信息 0x60-0x6F ===

/// 读取调度 tick 计数。
pub const SYS_TICKS: u64 = 0x60;

/// 读取单调时间戳（TSC）。
pub const SYS_TSC: u64 = 0x61;

/// 读取时间戳频率。
pub const SYS_TSC_FREQ: u64 = 0x62;

/// 读取民用时间。
pub const SYS_CIVIL_TIME: u64 = 0x63;

/// 读取平台启动命令行。
pub const SYS_BOOT_CMDLINE: u64 = 0x64;

/// 只读内核运行统计：线程/对象/就绪线程/trace 事件计数。
pub const SYS_KERNEL_STATS: u64 = 0x65;

// === 控制台与帧缓冲 0x70-0x7F ===

/// 把一段 UTF-8 文本写入内核控制台。
pub const SYS_WRITE: u64 = 0x70;

/// 非阻塞读取一个控制台字节；无输入时返回 `u64::MAX`。
pub const SYS_CONSOLE_READ: u64 = 0x71;

/// 注册为控制台输入订阅者。
pub const SYS_CONSOLE_SUBSCRIBE: u64 = 0x72;

/// 查询帧缓冲描述；仅显示服务可用。
pub const SYS_FRAMEBUFFER_INFO: u64 = 0x73;

/// 把帧缓冲映射到调用者地址空间；仅显示服务可用。
pub const SYS_FRAMEBUFFER_MAP: u64 = 0x74;

// === Root 与调试控制 0x80-0x8F ===

/// Root Server 控制请求；返回请求编号。
pub const SYS_ROOT_CONTROL: u64 = 0x80;

/// 读取 Root Server 控制结果；未完成时返回 `u64::MAX`。
pub const SYS_ROOT_RESULT: u64 = 0x81;

/// 注册端点接收内核投递的原始异常现场。
pub const SYS_FAULT_SUBSCRIBE: u64 = 0x82;

/// 带字节载荷的 Root 控制请求。
pub const SYS_ROOT_CONTROL_DATA: u64 = 0x83;

// === Root 控制操作码（`SYS_ROOT_CONTROL` 请求内的二级编号）===

/// Root 控制：任务视图。
pub const ROOT_OP_TASKS: u64 = 1;
/// Root 控制：服务视图。
pub const ROOT_OP_SERVICES: u64 = 2;
/// Root 控制：trace 视图。
pub const ROOT_OP_TRACE: u64 = 3;
/// Root 控制：运行时统计。
pub const ROOT_OP_STATS: u64 = 4;
/// Root 控制：启动服务。
pub const ROOT_OP_START: u64 = 5;
/// Root 控制：重启服务。
pub const ROOT_OP_RESTART: u64 = 6;
/// Root 控制：读取 HolyShell 脚本。
pub const ROOT_OP_SCRIPT: u64 = 7;
/// Root 控制：Handle 计数视图。
pub const ROOT_OP_HANDLES: u64 = 8;
/// Root 控制：符号化地址列表。
pub const ROOT_OP_SYMBOLIZE: u64 = 9;
/// Root 控制：用另一原生镜像热替换服务。
pub const ROOT_OP_REPLACE: u64 = 11;
/// Root 控制：保存 Native Task 检查点。
pub const ROOT_OP_CHECKPOINT: u64 = 12;
/// Root 控制：恢复 Native Task 检查点。
pub const ROOT_OP_RESTORE_CHECKPOINT: u64 = 13;
/// Root 控制：按服务与地址批量查询符号；服务为空时查内核符号。
pub const ROOT_OP_LOOKUP: u64 = 14;
/// Root 控制：读取检查点镜像片段；参数为 `任务 偏移`。
pub const ROOT_OP_CHECKPOINT_READ: u64 = 15;
/// Root 控制：写入检查点镜像片段；参数为 `任务 总长`。
pub const ROOT_OP_CHECKPOINT_WRITE: u64 = 16;
/// Root 控制：性能快照（每 CPU 忙时与每任务 CPU/内存）。
pub const ROOT_OP_PERF: u64 = 17;
/// 查看任务 Cadence。
pub const ROOT_OP_CADENCE: u64 = 18;
/// 查看任务 Meter。
pub const ROOT_OP_METER: u64 = 19;
/// 查看 Tempo 策略池。
pub const ROOT_OP_TEMPO: u64 = 20;

// === 服务名与内核投递标签 ===

/// Memory Server 名称；只有该服务可以申请物理页。
pub const MEMORY_SERVICE: &str = "hk-server-memory";

/// 显示服务名称；只有该服务可以访问帧缓冲。
pub const DISPLAY_SERVICE: &str = "hk-server-display";

/// 内核投递原始异常现场使用的消息标签。
pub const FAULT_EVENT: u64 = 0xD00;

/// 内核发给 Provider 的缺页请求标签；属于 `hk-proto::labels::PROVIDER` 段。
pub const PROVIDER_FAULT: u64 = 0x1200;

/// Provider 回复缺页请求的标签。
pub const PROVIDER_PROVIDE: u64 = 0x1201;

/// 帧缓冲描述符编码长度。
pub const FRAMEBUFFER_DESCRIPTOR_BYTES: usize = 40;

/// 帧缓冲描述符魔数。
pub const FRAMEBUFFER_MAGIC: u32 = 0x484B_4642;

/// 像素格式编码：32 位 RGB。
pub const PIXEL_FORMAT_RGB32: u32 = 0;
/// 像素格式编码：32 位 BGR。
pub const PIXEL_FORMAT_BGR32: u32 = 1;

/// 帧缓冲描述符。
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FramebufferDescriptor {
    /// 固定魔数。
    pub magic: u32,
    /// 像素格式编码。
    pub format: u32,
    /// 像素宽度。
    pub width: u32,
    /// 像素高度。
    pub height: u32,
    /// 每行字节数。
    pub stride: u32,
    /// 保留字段。
    pub reserved: u32,
    /// 起始物理地址。
    pub phys: u64,
    /// 总字节数。
    pub length: u64,
}

impl FramebufferDescriptor {
    /// 把描述符编码为固定长度字节串。
    pub fn encode(&self, out: &mut [u8; FRAMEBUFFER_DESCRIPTOR_BYTES]) {
        out[0..4].copy_from_slice(&self.magic.to_le_bytes());
        out[4..8].copy_from_slice(&self.format.to_le_bytes());
        out[8..12].copy_from_slice(&self.width.to_le_bytes());
        out[12..16].copy_from_slice(&self.height.to_le_bytes());
        out[16..20].copy_from_slice(&self.stride.to_le_bytes());
        out[20..24].copy_from_slice(&self.reserved.to_le_bytes());
        out[24..32].copy_from_slice(&self.phys.to_le_bytes());
        out[32..40].copy_from_slice(&self.length.to_le_bytes());
    }

    /// 从字节串解码；魔数不符时返回 `None`。
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < FRAMEBUFFER_DESCRIPTOR_BYTES {
            return None;
        }
        let word = |offset: usize| {
            u32::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ])
        };
        let wide = |offset: usize| {
            u64::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
                bytes[offset + 4],
                bytes[offset + 5],
                bytes[offset + 6],
                bytes[offset + 7],
            ])
        };
        let descriptor = Self {
            magic: word(0),
            format: word(4),
            width: word(8),
            height: word(12),
            stride: word(16),
            reserved: word(20),
            phys: wide(24),
            length: wide(32),
        };
        (descriptor.magic == FRAMEBUFFER_MAGIC).then_some(descriptor)
    }
}

/// 通知位：有控制台输入。
pub const CONSOLE_NOTIFY_INPUT: u64 = 1 << 0;
/// 通知位：控制台收到消息。
pub const CONSOLE_NOTIFY_MESSAGE: u64 = 1 << 1;

/// 句柄权限位：读取。
pub const RIGHT_READ: u32 = 1 << 0;
/// 句柄权限位：写入。
pub const RIGHT_WRITE: u32 = 1 << 1;
/// 句柄权限位：执行。
pub const RIGHT_EXECUTE: u32 = 1 << 2;
/// 句柄权限位：转移。
pub const RIGHT_TRANSFER: u32 = 1 << 3;
/// 句柄权限位：管理。
pub const RIGHT_ADMIN: u32 = 1 << 4;

/// Region 标志位：写时复制。
pub const REGION_FLAG_COW: u32 = 1 << 0;
/// Region 标志位：guard 页。
pub const REGION_FLAG_GUARD: u32 = 1 << 1;
/// Region 标志位：固定驻留。
pub const REGION_FLAG_PINNED: u32 = 1 << 2;
/// Region 标志位：只读共享。
pub const REGION_FLAG_SEALED: u32 = 1 << 3;
/// Region 标志位：fork 时不复制。
pub const REGION_FLAG_DONT_FORK: u32 = 1 << 4;
/// Region 标志位：可按需增长。
pub const REGION_FLAG_GROW: u32 = 1 << 5;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syscall_numbers_stay_in_their_groups() {
        const { assert!(SYS_THREAD_SET_TLS < 0x10 && SYS_CADENCE_SELF < 0x10) };
        const { assert!(SYS_HANDLE_CLOSE >= 0x10 && SYS_HANDLE_DUPLICATE < 0x20) };
        const { assert!(SYS_SEND >= 0x20 && SYS_REPLY_HANDLE < 0x40) };
        const { assert!(SYS_FUTEX_WAIT >= 0x30 && SYS_FUTEX_WAKE < 0x40) };
        const { assert!(SYS_ALLOC >= 0x40 && SYS_LEDGER_INFO < 0x50) };
        const { assert!(SYS_PCI_READ >= 0x50 && SYS_MSI_BIND < 0x60) };
        const { assert!(SYS_TICKS >= 0x60 && SYS_KERNEL_STATS < 0x70) };
        const { assert!(SYS_WRITE >= 0x70 && SYS_FRAMEBUFFER_MAP < 0x80) };
        const { assert!(SYS_ROOT_CONTROL >= 0x80 && SYS_FAULT_SUBSCRIBE < 0x90) };
        const { assert!(SYS_ARENA_INFO >= 0x90 && SYS_REGION_GROW < 0xA0) };
    }

    #[test]
    fn capability_kind_roundtrips_through_abi_encoding() {
        let kinds = [
            CapabilityKind::Revision,
            CapabilityKind::Surface,
            CapabilityKind::TaskImage,
            CapabilityKind::DeviceAllocation,
            CapabilityKind::Anonymous,
            CapabilityKind::Frame,
        ];
        for (index, kind) in kinds.iter().enumerate() {
            assert_eq!(kind.as_u8() as usize, index);
            assert_eq!(CapabilityKind::from_u8(index as u8), Some(*kind));
        }
        assert_eq!(CapabilityKind::from_u8(6), None);
    }

    #[test]
    fn syscall_numbers_are_unique() {
        let numbers = [
            SYS_EXIT,
            SYS_YIELD,
            SYS_FORK,
            SYS_THREAD_CREATE,
            SYS_THREAD_JOIN,
            SYS_THREAD_SET_TLS,
            SYS_SLEEP,
            SYS_CADENCE_INFO,
            SYS_CADENCE_UPDATE,
            SYS_CADENCE_SPLIT,
            SYS_CADENCE_REVOKE,
            SYS_METER_INFO,
            SYS_TASK_ATTACH_CADENCE,
            SYS_CADENCE_SELF,
            SYS_TASK_ATTACH_NAMED,
            SYS_TASK_REGRANT_NAMED,
            SYS_HANDLE_CLOSE,
            SYS_HANDLE_COUNT,
            SYS_HANDLE_DUPLICATE,
            SYS_SEND,
            SYS_SEND_HANDLE,
            SYS_SEND_DATA,
            SYS_RECV,
            SYS_RECV_DATA,
            SYS_RECV_DATA_FROM,
            SYS_RECV_DATA_HANDLE,
            SYS_RECV_DATA_TRY,
            SYS_RECV_DATA_HANDLE_TRY,
            SYS_RECV_DATA_TIMEOUT,
            SYS_CALL,
            SYS_CALL_TIMEOUT,
            SYS_CADENCE_NAMED,
            SYS_REPLY,
            SYS_REPLY_HANDLE,
            SYS_NOTIFY,
            SYS_WAIT_NOTIFY,
            SYS_WAIT_NOTIFY_TRY,
            SYS_SELF_NOTIFICATION,
            SYS_SERVICE_ENDPOINT,
            SYS_FUTEX_WAIT,
            SYS_FUTEX_WAKE,
            SYS_ALLOC,
            SYS_CAPABILITY_CREATE,
            SYS_REGION_MAP,
            SYS_REGION_INFO,
            SYS_REGION_UNMAP,
            SYS_CAPABILITY_PHYS,
            SYS_REGION_PROTECT,
            SYS_DMA_MAP_CAPABILITY,
            SYS_DMA_SYNC,
            SYS_CAPABILITY_PIN,
            SYS_CAPABILITY_SCANOUT,
            SYS_CAPABILITY_COMMIT,
            SYS_CAPABILITY_EXPORT,
            SYS_CAPABILITY_BIND_PROVIDER,
            SYS_CAPABILITY_CREATE_LAZY,
            SYS_LEDGER_INFO,
            SYS_PCI_READ,
            SYS_PCI_WRITE,
            SYS_DEVICE_GRANT_CREATE,
            SYS_DEVICE_MAP,
            SYS_DEVICE_INFO,
            SYS_IRQ_BIND,
            SYS_MSI_BIND,
            SYS_ACPI_LISTEN,
            SYS_ACPI_EVENTS,
            SYS_THERMAL,
            SYS_CPU_TOPOLOGY,
            SYS_TICKS,
            SYS_TSC,
            SYS_TSC_FREQ,
            SYS_CIVIL_TIME,
            SYS_BOOT_CMDLINE,
            SYS_KERNEL_STATS,
            SYS_WRITE,
            SYS_CONSOLE_READ,
            SYS_CONSOLE_SUBSCRIBE,
            SYS_FRAMEBUFFER_INFO,
            SYS_FRAMEBUFFER_MAP,
            SYS_ROOT_CONTROL,
            SYS_ROOT_RESULT,
            SYS_FAULT_SUBSCRIBE,
            SYS_ROOT_CONTROL_DATA,
            SYS_ARENA_INFO,
            SYS_CAPACITY_INFO,
            SYS_CAPACITY_SPLIT,
            SYS_CAPACITY_ALLOC,
            SYS_CAPACITY_FREE,
            SYS_FRAME_INFO,
            SYS_FRAME_PIN,
            SYS_FRAME_SHARE,
            SYS_FRAME_RELEASE,
            SYS_FRAME_FREE,
            SYS_FRAME_MAP,
            SYS_DMA_MAP_FRAME,
            SYS_MEMORY_STATS,
            SYS_CAPACITY_MERGE,
            SYS_LEDGER_ENUM,
            SYS_REGION_GROW,
        ];
        let mut sorted = numbers;
        sorted.sort_unstable();
        assert!(sorted.windows(2).all(|pair| pair[0] != pair[1]));
    }

    #[test]
    fn framebuffer_descriptor_roundtrip() {
        let descriptor = FramebufferDescriptor {
            magic: FRAMEBUFFER_MAGIC,
            format: PIXEL_FORMAT_RGB32,
            width: 640,
            height: 480,
            stride: 2560,
            reserved: 0,
            phys: 0x8000_0000,
            length: 1_228_800,
        };
        let mut bytes = [0_u8; FRAMEBUFFER_DESCRIPTOR_BYTES];
        descriptor.encode(&mut bytes);
        assert_eq!(FramebufferDescriptor::decode(&bytes), Some(descriptor));

        bytes[0] ^= 0xFF;
        assert_eq!(FramebufferDescriptor::decode(&bytes), None);
    }
}
