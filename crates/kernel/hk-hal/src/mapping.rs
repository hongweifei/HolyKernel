//! 页表映射属性。

bitflags::bitflags! {
    /// 映射属性。
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct MapFlags: u32 {
        /// 可读。
        const READ = 1 << 0;
        /// 可写。
        const WRITE = 1 << 1;
        /// 可执行。
        const EXECUTE = 1 << 2;
        /// 设备内存，使用非缓存属性。
        ///
        /// 内核与启动期映射用这一位即固定非缓存；用户态设备映射的缓存属性由
        /// [`CachePolicy`] 决定，平台在自己的映射入口选择，不经过本标志。
        const DEVICE = 1 << 3;
        /// 保护页，不映射物理内存。
        const GUARD = 1 << 4;
        /// 与其他地址空间共享。
        const SHARED = 1 << 5;
    }
}

impl MapFlags {
    /// 判断是否同时要求可写与可执行。
    ///
    /// W^X 要求可写与可执行互斥，映射前必须检查该条件。
    pub const fn violates_wx(&self) -> bool {
        self.bits() & Self::WRITE.bits() != 0 && self.bits() & Self::EXECUTE.bits() != 0
    }
}

/// 设备映射的缓存策略。
///
/// 缓属性属于映射种类而不是调用者请求：谁把物理区间交给任务，谁就知道它是
/// 寄存器窗口、系统内存扫描输出还是显存（ADR-0016 §115）。因此它不出现在
/// [`MapFlags`] 里，由平台在建立设备映射时选择。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CachePolicy {
    /// 写回：与普通内存同域。适配 CPU 与设备共享缓存一致性的系统内存，
    /// 例如 QEMU 的线性帧缓冲；写者不需要 `clflush`。
    WriteBack,
    /// 写合并：顺序写由 CPU 攒成整行突发，适合逐行写入的扫描输出缓冲。
    /// 读回代价高，不确定时不要选。
    WriteCombining,
    /// 非缓存：每次访问直达设备，适合寄存器窗口与必须逐次可见的控制面。
    Uncached,
}
