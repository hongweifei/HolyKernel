//! 物理页后端：全系统唯一的物理空闲表与账本。
//!
//! HMP `Capacity` 是后端之上的配额视图：它声明用途、页大小与配额，物理页
//! 取自这里。后端把每次分配归因到 `CapacityId`，因此“每个容量用了多少物理
//! 内存”只有一份权威记录，不再有并行的第二本账。
//!
//! 未注册后端时 `Capacity` 退回自带的窗口分配器，供单元测试与 Hosted 使用。

use hk_hal::PhysRange;

use super::{CapacityId, CapacityKind};

/// 物理页分配后端。
pub trait PhysicalBackend: Send + Sync {
    /// 把一段尚未归属的物理区间登记给指定容量。
    ///
    /// 区间必须完整落在后端的空闲空间内且不与其它容量重叠。登记只改变归属，
    /// 不改变空闲状态。
    fn assign(&self, owner: CapacityId, range: PhysRange) -> bool;

    /// 从父容量的归属区间中切出一段给子容量；返回切出的区间。
    fn split_owned(&self, parent: CapacityId, child: CapacityId, len: u64) -> Option<PhysRange>;

    /// 把子容量的归属区间并回父容量。
    fn merge_owned(&self, parent: CapacityId, child: CapacityId, range: PhysRange);

    /// 从指定容量的归属区间中分配连续物理页。
    fn allocate_owned(&self, owner: CapacityId, kind: CapacityKind, len: u64) -> Option<PhysRange>;

    /// 从指定容量名下归还一段物理区间。
    fn release_owned(&self, owner: CapacityId, range: PhysRange) -> bool;

    /// 返回指定容量名下的已用字节数。
    fn used_owned(&self, owner: CapacityId) -> u64;
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
