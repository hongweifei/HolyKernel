//! 物理容量能力。

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use hk_hal::{PhysAddr, PhysRange};
use spin::Mutex;

use super::{Frame, HmpError};

static NEXT_CAPACITY_ID: AtomicU64 = AtomicU64::new(1);

/// 归还物理区间前的清零回调。
static SCRUBBER: AtomicUsize = AtomicUsize::new(0);

/// 注册归还物理区间前的清零回调；重复调用会替换旧回调。
///
/// 内核在启动期注册基于 direct map 的实现；Hosted 等无物理内存的平台不注册，
/// 归还路径跳过清零。
pub fn set_frame_scrubber(scrub: fn(PhysAddr, usize)) {
    SCRUBBER.store(scrub as usize, Ordering::Release);
}

/// 归还物理区间前清零；未注册回调时跳过。
pub fn scrub(phys: PhysAddr, len: usize) {
    let scrub = SCRUBBER.load(Ordering::Acquire);
    if scrub == 0 {
        return;
    }
    // SAFETY: 回调由平台在启动期注册，签名与存储的地址一致。
    let scrub: fn(PhysAddr, usize) = unsafe { core::mem::transmute(scrub) };
    scrub(phys, len);
}

/// Capacity 标识。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapacityId(u64);

impl CapacityId {
    /// 构造标识。
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// 分配一个进程内唯一标识。
    pub fn next() -> Self {
        Self(NEXT_CAPACITY_ID.fetch_add(1, Ordering::Relaxed))
    }

    /// 返回标识数值。
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Capacity 用途。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapacityKind {
    /// 内核系统容量。
    System,
    /// 用户任务容量。
    User,
    /// DMA 容量。
    Dma,
    /// GPU/显存容量。
    Gpu,
    /// NUMA 节点容量。
    Numa(u8),
}

impl CapacityKind {
    /// 用途分类数量；统计数组按 System、User、Dma、Gpu、Numa 排列。
    pub const COUNT: usize = hk_syscall::CAPACITY_KIND_COUNT;

    /// 返回统计数组下标；所有 NUMA 节点归入同一档。
    pub const fn index(self) -> usize {
        match self {
            Self::System => 0,
            Self::User => 1,
            Self::Dma => 2,
            Self::Gpu => 3,
            Self::Numa(_) => 4,
        }
    }
}

/// 配额计数单元：子容量析构时把剩余配额并回父容量。
struct LimitCell {
    value: AtomicU64,
    parent: Mutex<Option<Arc<LimitCell>>>,
}

impl LimitCell {
    fn new(value: u64) -> Self {
        Self {
            value: AtomicU64::new(value),
            parent: Mutex::new(None),
        }
    }

    fn get(&self) -> u64 {
        self.value.load(Ordering::Acquire)
    }

    fn set(&self, value: u64) {
        self.value.store(value, Ordering::Release);
    }

    fn add(&self, amount: u64) {
        self.value.fetch_add(amount, Ordering::AcqRel);
    }

    /// 挂到父单元之下：析构时剩余配额沿链并回父容量。
    fn link_to(&self, parent: &Arc<LimitCell>) {
        *self.parent.lock() = Some(Arc::clone(parent));
    }
}

impl Drop for LimitCell {
    fn drop(&mut self) {
        let value = self.value.load(Ordering::Acquire);
        if value == 0 {
            return;
        }
        if let Some(parent) = self.parent.lock().as_ref() {
            parent.add(value);
        }
    }
}

/// 未注册后端时的窗口池：跨代共享，子池析构时把剩余窗口并回父池。
struct Pool {
    free: Vec<PhysRange>,
    next: u64,
    end: u64,
    parent: Option<Arc<Mutex<Pool>>>,
}

impl Pool {
    fn new(start: u64, length: u64) -> Self {
        Self {
            free: Vec::new(),
            next: start,
            end: start.saturating_add(length),
            parent: None,
        }
    }

    fn link_to(&mut self, parent: &Arc<Mutex<Pool>>) {
        self.parent = Some(Arc::clone(parent));
    }

    fn take(&mut self, length: u64, align: u64) -> Option<PhysRange> {
        let align = align.max(1);
        for index in 0..self.free.len() {
            let range = self.free[index];
            let start = align_up_u64(range.start().get(), align)?;
            let end = start.checked_add(length)?;
            if end > range.end().get() {
                continue;
            }
            let slot = self.free.swap_remove(index);
            if slot.start().get() < start {
                self.free.push(PhysRange::new(
                    slot.start(),
                    start.saturating_sub(slot.start().get()),
                ));
            }
            if end < slot.end().get() {
                self.free
                    .push(PhysRange::new(PhysAddr::new(end), slot.end().get() - end));
            }
            return Some(PhysRange::new(PhysAddr::new(start), length));
        }
        let start = align_up_u64(self.next, align)?;
        let end = start.checked_add(length)?;
        if end > self.end {
            return None;
        }
        self.next = end;
        Some(PhysRange::new(PhysAddr::new(start), length))
    }

    fn release(&mut self, range: PhysRange) {
        if range.is_empty() {
            return;
        }
        self.free.push(range);
        self.coalesce();
    }

    fn coalesce(&mut self) {
        self.free.sort_unstable_by_key(|range| range.start().get());
        let mut merged: Vec<PhysRange> = Vec::with_capacity(self.free.len());
        for range in self.free.drain(..) {
            if let Some(last) = merged.last_mut()
                && last.end().get() == range.start().get()
            {
                *last = PhysRange::new(last.start(), last.len().saturating_add(range.len()));
                continue;
            }
            merged.push(range);
        }
        self.free = merged;
    }

    /// 取出本池拥有的全部窗口（空闲区间 + 未分配的尾部）。
    fn drain_window(&mut self) -> Vec<PhysRange> {
        let mut windows: Vec<PhysRange> = core::mem::take(&mut self.free);
        if self.next < self.end {
            windows.push(PhysRange::new(
                PhysAddr::new(self.next),
                self.end - self.next,
            ));
            self.next = self.end;
        }
        windows
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        let windows = self.drain_window();
        if let Some(parent) = self.parent.as_ref() {
            let mut parent = parent.lock();
            for range in windows {
                parent.release(range);
            }
        }
    }
}

/// Root Server 授予的物理容量能力。
pub struct Capacity {
    id: CapacityId,
    range: PhysRange,
    page_size: usize,
    kind: CapacityKind,
    /// 由哪个容量 `split` 而来；根容量为 `None`。
    parent: Option<CapacityId>,
    /// 配额与可用窗口：父子共享同一条链，子容量析构时自动并回父容量。
    limit: Arc<LimitCell>,
    soft_limit: u64,
    used: u64,
    peak: u64,
    /// 未注册后端时的窗口池；注册后端时只承载回收链。
    pool: Arc<Mutex<Pool>>,
    /// 未注册后端时已分配的区间；用于校验归还归属与阻止重复释放。
    allocated: Vec<PhysRange>,
    /// 已通过 `merge` 显式并回父容量；析构时不再重复归还。
    released: bool,
    /// 绑定的 NUMA 节点；`None` 表示可从任意节点分配。
    node: Option<u8>,
}

impl Capacity {
    /// 以物理区间、页大小和配额创建 Capacity。
    pub fn new(
        id: CapacityId,
        range: PhysRange,
        page_size: usize,
        kind: CapacityKind,
        limit: u64,
    ) -> Result<Self, HmpError> {
        let available = range.len();
        let limit = if limit == 0 {
            available
        } else {
            limit.min(available)
        };
        Self::with_limit(id, range, page_size, kind, limit)
    }

    /// 以显式配额创建容量，不把配额裁剪到单个区间长度。
    ///
    /// 用于覆盖多个物理窗口的 System Capacity：`range` 只是窗口的外接范围，
    /// 配额按全部窗口总量声明。物理分配仍由后端按 `CapacityId` 的窗口归因。
    pub fn with_limit(
        id: CapacityId,
        range: PhysRange,
        page_size: usize,
        kind: CapacityKind,
        limit: u64,
    ) -> Result<Self, HmpError> {
        if page_size == 0 || !page_size.is_power_of_two() || range.is_empty() {
            return Err(HmpError::InvalidSize);
        }
        // 容量窗口只需按最小页对齐；更大的 page_size 由 Frame 分配时对齐。
        if !range.start().get().is_multiple_of(hk_hal::PAGE_SIZE as u64) {
            return Err(HmpError::Unaligned);
        }
        if limit < page_size as u64 {
            return Err(HmpError::InvalidSize);
        }
        let capacity = Self {
            id,
            range,
            page_size,
            kind,
            parent: None,
            limit: Arc::new(LimitCell::new(limit)),
            soft_limit: 0,
            used: 0,
            peak: 0,
            pool: Arc::new(Mutex::new(Pool::new(range.start().get(), limit))),
            allocated: Vec::new(),
            released: false,
            node: match kind {
                CapacityKind::Numa(node) => Some(node),
                _ => None,
            },
        };
        if let Some(backend) = super::backend::physical_backend() {
            backend.register_ledger(id, limit);
            backend.set_ledger_kind(id, kind);
        }
        Ok(capacity)
    }

    /// 分配一段连续物理页，返回 Frame。
    ///
    /// 注册了物理后端时由后端统一分配并按 `id` 归因；否则退回本容量自带的
    /// 窗口分配器（单元测试与 Hosted）。
    pub fn allocate(&mut self, len: usize) -> Result<Frame, HmpError> {
        self.allocate_on(len, None)
    }

    /// 按 NUMA 节点偏好分配；与容量绑定的节点冲突时直接失败。
    pub fn allocate_on(&mut self, len: usize, node: Option<u8>) -> Result<Frame, HmpError> {
        let node = match (self.node, node) {
            (Some(bound), Some(requested)) if bound != requested => {
                return Err(HmpError::OutOfCapacity);
            }
            (Some(bound), _) => Some(bound),
            (None, requested) => requested,
        };
        let length = align_up(len, self.page_size).ok_or(HmpError::InvalidSize)?;
        if length == 0 {
            return Err(HmpError::InvalidSize);
        }

        if let Some(backend) = super::backend::physical_backend() {
            // 子容量配额是硬上限；扩窗只解决窗口碎片，不扩大授予额。
            if self.used().saturating_add(length) > self.limit() {
                return Err(HmpError::OutOfCapacity);
            }
            if let Some(range) = backend.allocate_owned_aligned(
                self.id,
                self.kind,
                length,
                self.page_size as u64,
                node,
            ) {
                return Frame::new(range.start(), range.len() as usize, self.page_size, self.id);
            }
            // 配额尚有余量但既有窗口里没有足够长的连续空闲段：从父容量再扩
            // 一段连续窗口后重试一次。碎片化不能伪装成容量耗尽。
            if !self.grow(length) {
                return Err(HmpError::OutOfCapacity);
            }
            let range = backend
                .allocate_owned_aligned(
                    self.id,
                    self.kind,
                    length,
                    self.page_size as u64,
                    self.node,
                )
                .ok_or(HmpError::OutOfCapacity)?;
            return Frame::new(range.start(), range.len() as usize, self.page_size, self.id);
        }

        if self.used.saturating_add(length) > self.limit() {
            return Err(HmpError::OutOfCapacity);
        }

        let Some(range) = self.pool.lock().take(length, self.page_size as u64) else {
            return Err(HmpError::OutOfCapacity);
        };

        let frame = Frame::new(range.start(), length as usize, self.page_size, self.id)?;
        self.allocated.push(range);
        self.used = self.used.saturating_add(length);
        self.peak = self.peak.max(self.used);
        Ok(frame)
    }

    /// 从父 Capacity 动态扩出一段窗口，供当前容量继续分配。
    ///
    /// 只对通过 `split` 建立的子容量有效；扩窗只转移物理窗口的归属，用来
    /// 消除碎片，不改变任何一侧的配额。子容量的授予额在 `split` 时已经
    /// 固定，`grow` 不得让它越过父容量授予的范围。
    fn grow(&mut self, length: u64) -> bool {
        let Some(parent) = self.parent else {
            return false;
        };
        let Some(backend) = super::backend::physical_backend() else {
            return false;
        };
        backend.split_owned(parent, self.id, length).is_some()
    }

    /// 从未分配尾部切出一个子 Capacity。
    ///
    /// 分割把尾部区间从当前 Capacity 的可用空间中移除；子 Capacity 从切出
    /// 区间的起点开始分配。注册了物理后端时由后端登记子容量的归属窗口。
    /// 两条路径的 `used` 都只统计已分配的 Frame，切出窗口本身不计入用量。
    pub fn split(
        &mut self,
        id: CapacityId,
        len: usize,
        kind: CapacityKind,
    ) -> Result<Self, HmpError> {
        self.split_with_page_size(id, len, kind, 0)
    }

    /// 按指定页大小切出子容量；`0` 表示继承父容量。
    ///
    /// 子容量窗口只需按 4 KiB 对齐；更大的页由 Frame 分配时对齐，因此页大小
    /// 不会因为窗口起点而悄悄降级。
    pub fn split_with_page_size(
        &mut self,
        id: CapacityId,
        len: usize,
        kind: CapacityKind,
        page_size: usize,
    ) -> Result<Self, HmpError> {
        let node = match kind {
            CapacityKind::Numa(node) => Some(node),
            _ => self.node,
        };
        self.split_with_page_size_and_node(id, len, kind, page_size, node)
    }

    /// 把父容量窗口内的空闲页尽量转移给子容量，覆盖多个 NUMA 节点。
    ///
    /// 顶层 User 容量用它拿到分散在多个节点的空闲窗口；后端不支持时退化为
    /// 尾部切分。
    pub fn split_free(
        &mut self,
        id: CapacityId,
        kind: CapacityKind,
        limit: u64,
    ) -> Result<Self, HmpError> {
        let limit = align_up(limit as usize, self.page_size).ok_or(HmpError::InvalidSize)?;
        if limit == 0 || self.limit() < limit {
            return Err(HmpError::OutOfCapacity);
        }
        // split_free_to 以 4 KiB 粒度搬运窗口；大页容量直接走普通 split，
        // 避免搬出小于子容量页大小的窗口后构造失败。
        if self.page_size != hk_hal::PAGE_SIZE {
            return self.split(
                id,
                usize::try_from(limit).map_err(|_| HmpError::InvalidSize)?,
                kind,
            );
        }
        if let Some(backend) = super::backend::physical_backend() {
            let (range, moved) = backend
                .split_free_to(self.id, id, limit)
                .ok_or(HmpError::OutOfCapacity)?;
            if moved == 0 {
                return Err(HmpError::OutOfCapacity);
            }
            let mut child = Self::new(id, range, self.page_size, kind, moved)?;
            child.parent = Some(self.id);
            child.limit.link_to(&self.limit);
            self.set_limit(self.limit().saturating_sub(moved));
            if self.kind != CapacityKind::System {
                backend.set_ledger_limit(self.id, self.limit());
            }
            return Ok(child);
        }
        let length = usize::try_from(limit).map_err(|_| HmpError::InvalidSize)?;
        self.split(id, length, kind)
    }

    /// 按页大小与 NUMA 节点切出子容量。
    ///
    /// `node = Some(n)` 时子容量只从该节点的物理窗口分配；后端没有节点拓扑
    /// 或该节点没有足够窗口时返回 `OutOfCapacity`，不会静默跨节点。
    pub fn split_with_page_size_and_node(
        &mut self,
        id: CapacityId,
        len: usize,
        kind: CapacityKind,
        page_size: usize,
        node: Option<u8>,
    ) -> Result<Self, HmpError> {
        let page_size = if page_size == 0 {
            self.page_size
        } else {
            page_size
        };
        if page_size < hk_hal::PAGE_SIZE || !page_size.is_power_of_two() {
            return Err(HmpError::InvalidSize);
        }
        let length = align_up(len, page_size).ok_or(HmpError::InvalidSize)?;
        if length == 0 {
            return Err(HmpError::InvalidSize);
        }

        if self.limit() < length {
            return Err(HmpError::OutOfCapacity);
        }
        if let Some(backend) = super::backend::physical_backend() {
            let range = match node {
                Some(node) => backend.split_owned_node(self.id, id, length, node),
                None => backend.split_owned(self.id, id, length),
            }
            .ok_or(HmpError::OutOfCapacity)?;
            let mut child = Self::new(id, range, page_size, kind, length)?;
            child.node = node.or(child.node);
            child.parent = Some(self.id);
            child.limit.link_to(&self.limit);
            self.set_limit(self.limit().saturating_sub(length));
            if self.kind != CapacityKind::System {
                backend.set_ledger_limit(self.id, self.limit());
            }
            return Ok(child);
        }

        let range = self
            .pool
            .lock()
            .take(length, page_size as u64)
            .ok_or(HmpError::OutOfCapacity)?;
        let mut child = Self::new(id, range, page_size, kind, length)?;
        child.node = node.or(child.node);
        child.parent = Some(self.id);
        child.limit.link_to(&self.limit);
        child.pool.lock().link_to(&self.pool);
        self.set_limit(self.limit().saturating_sub(length));
        Ok(child)
    }

    /// 归还 Frame 的物理区间。
    ///
    /// 只接受已无其他所有者的 Frame：引用计数为 1（仅持有者）或已归零
    /// （最后一个引用已被 `Frame::release` 释放）。仍被共享或固定的 Frame
    /// 拒绝归还，且 Frame 保留在调用方的槽位中，不会因错误路径丢失。
    pub fn free(&mut self, frame: &mut Option<Frame>) -> Result<(), HmpError> {
        let Some(inner) = frame.as_ref() else {
            return Ok(());
        };
        if inner.capacity() != self.id {
            return Err(HmpError::WrongCapability);
        }
        if inner.is_pinned() {
            return Err(HmpError::PermissionDenied);
        }
        let (phys, len) = (inner.phys(), inner.len());
        self.release(phys, len)?;
        frame.take();
        Ok(())
    }

    /// 归还一段原始物理区间，供底层 DMA 分配器使用。
    ///
    /// 归还前清零区间；敏感内容不会随 Frame 回到空闲池。注册了物理后端时
    /// 由后端统一归还并释放归因。
    pub fn release(&mut self, phys: PhysAddr, len: usize) -> Result<(), HmpError> {
        let length = align_up(len, self.page_size).ok_or(HmpError::InvalidSize)?;
        if length == 0 || !phys.get().is_multiple_of(self.page_size as u64) {
            return Err(HmpError::Unaligned);
        }
        // 注册了后端时由后端按分配记录校验归属；容量可能覆盖多个窗口，不能用
        // 单个区间做地址范围检查。后端在校验通过并移除分配记录后、把区间交回
        // 空闲表之前清零，避免先擦掉已经重新分配给其他租户的页。
        if let Some(backend) = super::backend::physical_backend() {
            return if backend.release_owned(self.id, PhysRange::new(phys, length)) {
                Ok(())
            } else {
                Err(HmpError::NotMapped)
            };
        }

        let start = phys.get();
        let end = start.checked_add(length).ok_or(HmpError::OutOfRange)?;
        let limit_end = self.range.start().get().saturating_add(self.limit());
        if start < self.range.start().get() || end > limit_end {
            return Err(HmpError::OutOfRange);
        }
        // fallback 没有后端分配记录，必须自证区间属于本容量且未归还过。
        let Some(index) = self
            .allocated
            .iter()
            .position(|range| range.start().get() == start && range.len() == length)
        else {
            return Err(HmpError::NotMapped);
        };
        self.allocated.swap_remove(index);
        scrub(phys, length as usize);

        self.used = self.used.saturating_sub(length);
        self.pool.lock().release(PhysRange::new(phys, length));
        Ok(())
    }

    /// 把一个已清空的子 Capacity 并回本容量。
    ///
    /// 子 Capacity 的 Frame 必须全部归还（权威用量为零），否则拒绝。只有
    /// 确认已清空才通知后端归还窗口，保证子容量名下的分配不会丢失归属。
    pub fn merge(&mut self, child: &mut Capacity) -> Result<(), HmpError> {
        if child.id == self.id {
            return Err(HmpError::WrongCapability);
        }
        if child.parent != Some(self.id) {
            return Err(HmpError::WrongCapability);
        }
        if child.released {
            return Err(HmpError::WrongCapability);
        }
        if child.used() != 0 {
            return Err(HmpError::PermissionDenied);
        }
        if child.range.start().get() < self.range.start().get() {
            return Err(HmpError::OutOfRange);
        }

        // 后端路径把窗口归属与后端 Ledger 一起并回父容量。
        if let Some(backend) = super::backend::physical_backend() {
            backend.reclaim_capacity(child.id, Some(self.id), child.limit())?;
        } else {
            // 子池当前剩余窗口立即并回父池；孙容量仍持有的窗口在各自池析构
            // 时沿父链归还，父容量不会提前分配这些区间。
            let windows = child.pool.lock().drain_window();
            {
                let mut pool = self.pool.lock();
                for window in windows {
                    pool.release(window);
                }
            }
            child.pool.lock().link_to(&self.pool);
        }
        // 配额立即并回父容量；子单元清零后挂到父单元，孙容量析构时的配额
        // 会沿链继续流回父容量。
        self.set_limit(self.limit().saturating_add(child.limit()));
        child.limit.set(0);
        child.limit.link_to(&self.limit);
        child.released = true;
        Ok(())
    }

    /// 返回 Capacity 标识。
    pub const fn id(&self) -> CapacityId {
        self.id
    }

    /// 返回首个物理窗口；完整窗口列表使用 [`Self::windows`]。
    pub const fn range(&self) -> PhysRange {
        self.range
    }

    /// 返回当前持有的全部物理窗口，按起始地址排序。
    ///
    /// 注册了物理后端时以窗口归属为权威；未注册后端时只有创建时的窗口。
    pub fn windows(&self) -> alloc::vec::Vec<PhysRange> {
        if let Some(backend) = super::backend::physical_backend() {
            // 后端是窗口归属的权威：已被 split/merge 清空时不伪造创建时的区间。
            return backend.windows_owned(self.id);
        }
        alloc::vec![self.range]
    }

    /// 返回页大小。
    pub const fn page_size(&self) -> usize {
        self.page_size
    }

    /// 返回绑定的 NUMA 节点；`None` 表示任意节点。
    pub const fn node(&self) -> Option<u8> {
        self.node
    }

    /// 返回用途。
    pub const fn kind(&self) -> CapacityKind {
        self.kind
    }

    /// 返回配额字节数。
    pub fn limit(&self) -> u64 {
        self.limit.get()
    }

    fn set_limit(&self, limit: u64) {
        self.limit.set(limit);
    }

    /// 设置软上限；0 表示未设置。软上限只用于报告，不拒绝分配。
    pub fn set_soft_limit(&mut self, limit: u64) {
        self.soft_limit = limit;
        if let Some(backend) = super::backend::physical_backend() {
            backend.set_ledger_soft_limit(self.id, limit);
        }
    }

    /// 返回软上限。
    pub const fn soft_limit(&self) -> u64 {
        self.soft_limit
    }

    /// 返回峰值用量。
    ///
    /// 注册了物理后端时读后端的按容量峰值记录；否则用本地峰值。
    pub fn peak(&self) -> u64 {
        if let Some(backend) = super::backend::physical_backend() {
            return backend.peak_owned(self.id);
        }
        self.peak
    }

    /// 返回压力百分比（0–100）。
    pub fn pressure(&self) -> u64 {
        let limit = self.limit();
        if limit == 0 {
            return 100;
        }
        self.used().saturating_mul(100) / limit
    }

    /// 返回已分配字节数。
    ///
    /// 注册了物理后端时直接读后端的归因记录，避免本地计数与权威账本分叉。
    pub fn used(&self) -> u64 {
        if let Some(backend) = super::backend::physical_backend() {
            return backend.used_owned(self.id);
        }
        self.used
    }
}

impl Drop for Capacity {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        // 后端路径归还窗口归属；配额与 fallback 窗口由 LimitCell/Pool 的
        // Drop 沿父子链自动并回，不在这里重复处理。
        if let Some(backend) = super::backend::physical_backend() {
            let _ = backend.reclaim_capacity_force(self.id, self.parent, self.limit());
        }
    }
}

fn align_up(value: usize, alignment: usize) -> Option<u64> {
    let value = u64::try_from(value).ok()?;
    let alignment = alignment as u64;
    let aligned = value.checked_add(alignment.checked_sub(1)?)? / alignment * alignment;
    (aligned != 0).then_some(aligned)
}

/// 64 位数值按 `align` 向上取整。
fn align_up_u64(value: u64, align: u64) -> Option<u64> {
    let align = align.max(1);
    value
        .checked_add(align - 1)
        .map(|value| value / align * align)
}

#[cfg(test)]
mod tests;
