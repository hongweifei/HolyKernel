//! 内核与服务器共享的系统调用 ABI。
//!
//! 只包含编号与端点约定，不依赖任何平台；寄存器映射由平台层实现。

#![no_std]

/// 退出服务。
pub const SYS_EXIT: u64 = 0;

/// 把一段 UTF-8 文本写入内核控制台。
pub const SYS_WRITE: u64 = 1;

/// 让出 CPU。
pub const SYS_YIELD: u64 = 2;

/// 向端点发送消息。
pub const SYS_SEND: u64 = 3;

/// 从端点接收消息。
pub const SYS_RECV: u64 = 4;

/// 读取 PCI 配置空间。
pub const SYS_PCI_READ: u64 = 5;

/// 读取调度 tick 计数。
pub const SYS_TICKS: u64 = 6;

/// 在任务 Arena 内分配内存。
pub const SYS_ALLOC: u64 = 7;

/// 关闭端点句柄。
pub const SYS_HANDLE_CLOSE: u64 = 8;

/// 查询在用句柄数。
pub const SYS_HANDLE_COUNT: u64 = 9;

/// 发送消息并转移一个句柄。
pub const SYS_SEND_HANDLE: u64 = 10;

/// 置位通知位。
pub const SYS_NOTIFY: u64 = 11;

/// 等待通知位。
pub const SYS_WAIT_NOTIFY: u64 = 12;

/// 调用端点并等待回复。
pub const SYS_CALL: u64 = 13;

/// 回复最近一次调用。
pub const SYS_REPLY: u64 = 14;

/// 复制当前任务。
pub const SYS_FORK: u64 = 15;

/// 读取单调时间戳（TSC）。
pub const SYS_TSC: u64 = 16;

/// 读取时间戳频率。
pub const SYS_TSC_FREQ: u64 = 17;

/// 发送带字节载荷的消息。
pub const SYS_SEND_DATA: u64 = 18;

/// 接收带字节载荷的消息。
pub const SYS_RECV_DATA: u64 = 19;

/// 写入 PCI 配置空间。
pub const SYS_PCI_WRITE: u64 = 20;

/// 创建 DeviceGrant 对象。
pub const SYS_DEVICE_GRANT_CREATE: u64 = 21;

/// 把 DeviceGrant 的 MMIO 范围映射到当前任务。
pub const SYS_DEVICE_MAP: u64 = 22;

/// 分配一致性 DMA 缓冲并映射到当前任务。
pub const SYS_DMA_ALLOC: u64 = 23;

/// 把设备中断绑定到通知。
pub const SYS_IRQ_BIND: u64 = 24;

/// 同步 DMA 缓冲缓存。
pub const SYS_DMA_SYNC: u64 = 26;

/// 接收带字节载荷的消息，并指定期望的发送端点。
pub const SYS_RECV_DATA_FROM: u64 = 27;

/// 按服务名查询当前任务端点。
pub const SYS_SERVICE_ENDPOINT: u64 = 28;

/// 查询 DeviceGrant 编码描述。
pub const SYS_DEVICE_INFO: u64 = 29;

/// 读取民用时间。
pub const SYS_CIVIL_TIME: u64 = 30;

/// 读取平台启动命令行。
pub const SYS_BOOT_CMDLINE: u64 = 31;

/// 分配共享 Frame 并映射。
pub const SYS_FRAME_ALLOC: u64 = 32;

/// 把 Frame 映射到调用者地址空间。
pub const SYS_FRAME_MAP: u64 = 33;

/// 解除 Frame 映射。
pub const SYS_FRAME_UNMAP: u64 = 34;

/// 查询 Frame 的物理地址与长度。
pub const SYS_FRAME_INFO: u64 = 35;

/// 复制句柄。
pub const SYS_HANDLE_DUPLICATE: u64 = 36;

/// 接收带字节载荷的消息，并把随消息收到的句柄编号打包返回。
pub const SYS_RECV_DATA_HANDLE: u64 = 37;

/// 把共享 Frame 绑定到 DeviceGrant 的 IOMMU 域。
pub const SYS_DMA_MAP_FRAME: u64 = 38;

/// 非阻塞接收带字节载荷的消息；无消息时返回 0。
pub const SYS_RECV_DATA_TRY: u64 = 39;

/// 非阻塞读取一个控制台字节；无输入时返回 `u64::MAX`。
pub const SYS_CONSOLE_READ: u64 = 40;
/// 注册为控制台输入订阅者。
pub const SYS_CONSOLE_SUBSCRIBE: u64 = 41;

/// 查询帧缓冲描述；仅显示服务可用。
pub const SYS_FRAMEBUFFER_INFO: u64 = 42;

/// 把帧缓冲映射到调用者地址空间；仅显示服务可用。
pub const SYS_FRAMEBUFFER_MAP: u64 = 43;

/// 非阻塞接收带字节载荷与句柄的消息；无消息时返回 0。
pub const SYS_RECV_DATA_HANDLE_TRY: u64 = 44;
/// 只读内核运行统计：线程/对象/就绪线程/trace 事件计数。
pub const SYS_KERNEL_STATS: u64 = 45;

/// Root Server 控制请求；返回请求编号。
pub const SYS_ROOT_CONTROL: u64 = 46;

/// 读取 Root Server 控制结果；未完成时返回 `u64::MAX`。
pub const SYS_ROOT_RESULT: u64 = 47;

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

/// 在同一地址空间创建线程。
pub const SYS_THREAD_CREATE: u64 = 48;
/// 等待线程退出。
pub const SYS_THREAD_JOIN: u64 = 49;
/// 设置当前线程的 TLS 基址。
pub const SYS_THREAD_SET_TLS: u64 = 50;
/// 在共享内存字上等待。
pub const SYS_FUTEX_WAIT: u64 = 51;
/// 唤醒共享内存字上的等待者。
pub const SYS_FUTEX_WAKE: u64 = 52;
/// `SYS_FUTEX_WAIT`：值已变化，无需等待。
pub const FUTEX_CHANGED: u64 = 1;

/// 注册端点接收内核投递的原始异常现场。
pub const SYS_FAULT_SUBSCRIBE: u64 = 53;

/// 内核投递原始异常现场使用的消息标签。
pub const FAULT_EVENT: u64 = 0xD00;

/// 帧缓冲描述符编码长度。
pub const FRAMEBUFFER_DESCRIPTOR_BYTES: usize = 40;

/// 帧缓冲描述符魔数。
pub const FRAMEBUFFER_MAGIC: u32 = 0x484B_4642;

/// 显示服务名称；只有该服务可以访问帧缓冲。
pub const DISPLAY_SERVICE: &str = "hk-server-display";

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
pub const RIGHT_READ: u64 = 1;
/// 句柄权限位：写入。
pub const RIGHT_WRITE: u64 = 2;
/// 句柄权限位：执行。
pub const RIGHT_EXECUTE: u64 = 4;
/// 句柄权限位：转移。
pub const RIGHT_TRANSFER: u64 = 8;
/// 句柄权限位：管理。
pub const RIGHT_ADMIN: u64 = 16;

/// 返回当前任务自己的通知编号。
pub const SYS_SELF_NOTIFICATION: u64 = 25;

/// echo 服务的端点编号。
pub const ENDPOINT_ECHO: u64 = 2;

/// 服务角色元数据，链接到镜像的 `.hk_service` 段。
#[repr(C)]
pub struct ServiceRole {
    /// 固定魔数。
    pub magic: u32,
    /// 角色位图。
    pub role: u32,
}

impl ServiceRole {
    /// 元数据魔数。
    pub const MAGIC: u32 = 0x484B_5352;
    /// 设备管理服务角色。
    pub const DEVICE: u32 = 1;

    /// 构建设备管理服务角色。
    pub const fn device() -> Self {
        Self {
            magic: Self::MAGIC,
            role: Self::DEVICE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
