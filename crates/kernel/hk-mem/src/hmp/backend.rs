//! 物理页后端：全系统唯一的物理空闲表与账本。
//!
//! HMP `Capacity` 是后端之上的配额视图：它声明用途、页大小与配额，物理页
//! 取自这里。后端把每次分配归因到 `CapacityId`，因此“每个容量用了多少物理
//! 内存”只有一份权威记录，不再有并行的第二本账。
//!
//! 未注册后端时 `Capacity` 退回自带的窗口分配器，供单元测试与 Hosted 使用。

use hk_hal::PhysRange;

use super::{CapacityId, CapacityKind, HmpError};

/// 物理页分配后端。
///
/// 窗口归属只由 `reserve_for` / `split_owned` / `merge_owned` 建立；分配与
/// 归还都必须在已归属的窗口内发生。
pub trait PhysicalBackend: Send + Sync {
    /// 从父容量的归属区间中切出一段给子容量；返回切出的区间。
    fn split_owned(&self, parent: CapacityId, child: CapacityId, len: u64) -> Option<PhysRange>;

    /// 把子容量已清空的归属区间并回父容量。
    ///
    /// 后端是归属窗口的权威：子容量仍有未归还的分配或窗口与容量不匹配时返回
    /// 错误，调用方据此拒绝合并，而不是在窗口仍留在后端时报告成功。
    fn merge_owned(
        &self,
        parent: CapacityId,
        child: CapacityId,
        range: PhysRange,
    ) -> Result<(), HmpError>;

    /// 从指定容量的归属区间中分配连续物理页。
    fn allocate_owned(&self, owner: CapacityId, kind: CapacityKind, len: u64) -> Option<PhysRange>;

    /// 从指定容量名下归还一段物理区间。
    fn release_owned(&self, owner: CapacityId, range: PhysRange) -> bool;

    /// 返回指定容量名下的已用字节数。
    fn used_owned(&self, owner: CapacityId) -> u64;

    /// 返回指定容量名下的峰值用量。
    fn peak_owned(&self, owner: CapacityId) -> u64;
}

static BACKEND: spin::Mutex<Option<&'static dyn PhysicalBackend>> = spin::Mutex::new(None);

/// 注册全局物理页后端；内核在启动期调用，重复调用会替换旧后端。
pub fn set_physical_backend(backend: &'static dyn PhysicalBackend) {
    *BACKEND.lock() = Some(backend);
}

/// 返回已注册的全局物理页后端。
pub fn physical_backend() -> Option<&'static dyn PhysicalBackend> {
    *BACKEND.lock()
}
