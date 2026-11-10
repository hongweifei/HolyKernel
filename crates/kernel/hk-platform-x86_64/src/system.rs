//! 启动期建立的 System Capacity。
//!
//! System Capacity 覆盖全部可用物理内存中尚未被别人预留的部分：DMA 分配器等已预留
//! 窗口保持各自归属，其余空闲区间登记给 System；内核堆、页表、镜像与匿名页都
//! 计入它的用量。User/Dma/Gpu 等授予用容量从它切出，容量树因此扎根在完整的
//! 启动内存描述上，而不是某个池的子集。

use alloc::sync::Arc;

use hk_hal::{PAGE_SIZE, PhysRange};
use hk_mem::FrameBank;
use hk_mem::hmp::{Capacity, CapacityId, CapacityKind};
use spin::Mutex;

static SYSTEM: Mutex<Option<Arc<Mutex<Capacity>>>> = Mutex::new(None);
static USER: Mutex<Option<Arc<Mutex<Capacity>>>> = Mutex::new(None);

/// 建立 System Capacity 并接管内核自用分配的归因。
///
/// `heap` 是启动期已分配的内核堆区间；`reserved` 是已预留给他人的字节数
/// （当前为 DMA 分配器）。没有剩余可用内存或内核堆记录不一致时返回 `None`。
pub fn establish(bank: &FrameBank, heap: PhysRange, reserved: u64) -> Option<()> {
    let id = CapacityId::next();
    let range = bank.reserve_remaining(id)?;
    let limit = bank.total().saturating_sub(reserved);
    // 先建立 System 自己的 Ledger，再接管内核堆，保证 adopt 的记账有账可落。
    // 账本上限用物理总量：内核自用不受用户 split 配额扣减影响；用户分配仍由
    // `Capacity::limit` 校验。
    bank.register_ledger(id, bank.total());
    if !bank.adopt_owner(id, heap) {
        return None;
    }
    // 启动早期还可能分配了缓存/页表等未归属区间；全部补入 System 账本。
    if !bank.adopt_all_unowned(id) {
        return None;
    }
    bank.set_kernel_owner(Some(id));
    let mut capacity =
        Capacity::with_limit(id, range, PAGE_SIZE, CapacityKind::System, limit).ok()?;
    // 顶层 User 容量从 System 切出，后续由 Capacity::grow 随用户分配增长。
    let user_id = CapacityId::next();
    let user = capacity
        .split(user_id, PAGE_SIZE, CapacityKind::User)
        .ok()?;
    *SYSTEM.lock() = Some(Arc::new(Mutex::new(capacity)));
    *USER.lock() = Some(Arc::new(Mutex::new(user)));
    Some(())
}

/// 返回共享的 System Capacity。
pub fn capacity() -> Option<Arc<Mutex<Capacity>>> {
    SYSTEM.lock().clone()
}

/// 返回 Root 建立、供 Memory Server 共享的顶层 User Capacity。
pub fn user_capacity() -> Option<Arc<Mutex<Capacity>>> {
    USER.lock().clone()
}

/// 从 System Capacity 尾部切出一段独立容量。
pub fn split_capacity(len: usize, kind: CapacityKind) -> Option<Capacity> {
    let system = capacity()?;
    let mut system = system.lock();
    system.split(CapacityId::next(), len, kind).ok()
}
