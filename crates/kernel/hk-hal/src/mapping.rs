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
