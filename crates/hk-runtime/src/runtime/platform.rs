//! 平台提供的用户态运行支持。

use hk_hal::CivilTime;

/// 平台提供的用户态运行支持。
pub trait UserPlatform {
    /// 返回调度 tick 计数。
    fn timer_ticks(&self) -> u64;

    /// 返回当前民用时间；没有 RTC 时返回 `None`。
    fn civil_time(&self) -> Option<CivilTime> {
        None
    }

    /// 返回平台启动命令行。
    fn boot_cmdline(&self) -> Option<&'static str> {
        None
    }
    /// 读取 PCI 配置空间中的一个 32 位字。
    fn pci_read(&self, bus: u8, device: u8, function: u8, offset: u8) -> u32;
    /// 写入 PCI 配置空间中的一个 32 位字。
    fn pci_write(&self, _bus: u8, _device: u8, _function: u8, _offset: u8, _value: u32) {}
    /// 把设备 MMIO 映射到任务地址空间。
    fn map_mmio(&self, _slot: usize, _virt: u64, _phys: u64, _len: usize) -> bool {
        false
    }
    /// 把 DMA 内存映射到任务地址空间。
    fn map_dma(&self, _slot: usize, _virt: u64, _phys: u64, _len: usize) -> bool {
        false
    }

    /// 解除任务地址空间中的一段映射。
    fn unmap_dma(&self, _slot: usize, _virt: u64, _len: usize) -> bool {
        false
    }
    /// 为 DeviceGrant 分配连续 DMA 窗口。
    fn dma_window(&self, _len: usize) -> Option<hk_hal::PhysRange> {
        None
    }
    /// 归还 DeviceGrant 的 DMA 窗口。
    fn dma_free(&self, _range: hk_hal::PhysRange) {}
    /// 取出并清除待处理 IRQ 位图，最多 256 条。
    fn take_irqs(&self, _pending: &mut [u64; 4]) {}
    /// 返回平台中断控制器。
    fn interrupt_controller(&self) -> Option<&dyn hk_hal::InterruptController> {
        None
    }
    /// 返回平台 IOMMU。
    fn iommu(&self) -> Option<&dyn hk_hal::Iommu> {
        None
    }
    /// 返回平台缓存维护接口。
    fn cache_ops(&self) -> Option<&dyn hk_hal::CacheOps> {
        None
    }
    /// 把用户地址区间清零。
    fn zero(&self, address: u64, length: usize);
    /// 非阻塞读取一个控制台字节。
    fn console_read(&self) -> Option<u8> {
        None
    }
    /// 取出并清除“有控制台输入”标志。
    fn take_console_input(&self) -> bool {
        false
    }
    /// 判断地址是否为当前任务可读用户页。
    fn user_mapped(&self, _address: u64) -> bool {
        false
    }
    /// 从当前任务用户内存读取字节；地址无效时返回 `false`。
    fn read_user(&self, _address: u64, _buffer: &mut [u8]) -> bool {
        false
    }
    /// 向当前任务用户内存写入字节；地址无效时返回 `false`。
    fn write_user(&self, _address: u64, _data: &[u8]) -> bool {
        false
    }
    /// 返回任务槽位的 Arena 基址与大小。
    fn arena_layout(&self, slot: usize) -> (u64, u64);
    /// 切换到任务槽位的地址空间。
    fn activate(&self, _slot: usize) {}
    /// 返回任务槽位的页表根；没有地址空间时为 0。
    fn vspace(&self, _slot: usize) -> u64 {
        0
    }
    /// 把父任务的用户地址空间复制为子任务的写时复制副本。
    fn fork(&self, _parent: usize, _child: usize) -> bool {
        false
    }
    /// 让子任务与父任务共享同一用户地址空间。
    fn share_vspace(&self, _parent: usize, _child: usize) -> bool {
        false
    }
    /// 设置当前 CPU 的线程指针（TLS 基址）。
    fn set_thread_pointer(&self, _tls: u64) {}
    /// 返回单调时间戳；无计时能力时为 0。
    fn timestamp(&self) -> u64 {
        0
    }
    /// 返回时间戳频率；未知时为 0。
    fn tsc_frequency(&self) -> u64 {
        0
    }
    /// 返回平台帧缓冲描述。
    fn framebuffer(&self) -> Option<hk_hal::FramebufferInfo> {
        None
    }
    /// 把帧缓冲映射到任务地址空间，返回映射长度。
    fn map_framebuffer(&self, _slot: usize, _virt: u64) -> Option<u64> {
        None
    }
    /// 唤醒空闲 CPU 重新检查调度队列。
    fn wake_aps(&self) {}
}
