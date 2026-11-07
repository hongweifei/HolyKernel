//! 用户地址空间映射接口。

/// 用户页映射。
///
/// 实现方分配一页、清零并以用户可写权限映射到 `virtual_address`，返回整页
/// 可写视图。装载器只在返回的切片内写入内容。
pub trait UserMemory {
    /// 映射一页并返回可写视图；物理帧耗尽或映射失败时返回 `None`。
    fn map_page(&mut self, virtual_address: u64) -> Option<&mut [u8]>;
}
