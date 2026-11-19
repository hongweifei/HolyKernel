//! 统一 Frame 引用与任务记账。
//!
//! 本模块把两件事拆开：
//!
//! - **映射引用**：`mapping_acquire` / `mapping_release` 只维护物理页被多少
//!   个页表映射。它决定 FrameBank 何时可以归还物理页，不参与 Task Ledger。
//! - **任务记账**：`charge` / `charge_range` 按 `(scope, 物理页)` 记入一个
//!   Task Ledger。每次返回的 [`FrameRef`] 是一份显式引用，释放必须与获取
//!   配对；映射引用独立维护。
//!
//! 这样同一物理页可以同时被多个任务映射、在多个任务账本中分别计费，而页表
//! 拆除只会影响映射引用，不会误释放另一个任务的账目。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use hk_hal::PhysRange;
use spin::Mutex;

use super::{Charge, HmpError, SharedLedger};

/// 全局物理帧账本：映射引用与任务记账共用同一对象，但使用两套独立计数。
static FRAME_LEDGER: Mutex<FrameLedger> = Mutex::new(FrameLedger::new());

/// 增加一次物理页映射引用。
pub fn mapping_acquire(phys: u64) -> bool {
    FRAME_LEDGER.lock().acquire_mapping(phys)
}

/// 释放一次物理页映射引用；返回剩余引用数。
pub fn mapping_release(phys: u64) -> u32 {
    FRAME_LEDGER.lock().release_mapping(phys)
}

/// 返回物理页当前的映射引用数。
pub fn mapping_count(phys: u64) -> u32 {
    FRAME_LEDGER
        .lock()
        .mapping_refs
        .get(&phys)
        .copied()
        .unwrap_or(0)
}

/// 清空全局映射引用；仅测试使用。
pub fn mapping_clear() {
    FRAME_LEDGER.lock().mapping_refs.clear();
}

/// 返回当前跟踪映射引用的物理页数。
pub fn mapping_tracked() -> usize {
    FRAME_LEDGER.lock().mapping_refs.len()
}

/// 分配一个记账作用域；每个进程一个，避免同一运行时的不同任务混账。
pub fn scope() -> u64 {
    let mut ledger = FRAME_LEDGER.lock();
    let scope = ledger.next_scope;
    ledger.next_scope = ledger.next_scope.saturating_add(1);
    scope
}

/// 记入一段无名常驻区间。
pub fn charge(scope: u64, ledger: &SharedLedger, amount: u64) -> Result<FrameRef, HmpError> {
    FRAME_LEDGER.lock().charge(scope, ledger, amount)
}

/// 记入一段物理区间；同一作用域内同一页只记一次。
pub fn charge_range(
    scope: u64,
    ledger: &SharedLedger,
    range: PhysRange,
) -> Result<FrameRef, HmpError> {
    FRAME_LEDGER.lock().charge_range(scope, ledger, range)
}

/// 接管一笔已经扣过的记账。
pub fn adopt(scope: u64, ledger: SharedLedger, charge: Charge) -> FrameRef {
    FRAME_LEDGER.lock().adopt(scope, ledger, charge)
}

/// 增加一次记账引用。
pub fn retain(frame: FrameRef) {
    FRAME_LEDGER.lock().retain(frame);
}

/// 释放一次记账引用。
pub fn release(frame: FrameRef) {
    FRAME_LEDGER.lock().release(frame);
}

/// 返回记账字节数。
pub fn amount(frame: FrameRef) -> u64 {
    FRAME_LEDGER.lock().amount(frame)
}

/// 常驻区间记账引用。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FrameRef {
    id: u64,
}

impl FrameRef {
    /// 构造引用；仅供账本内部与测试使用。
    pub const fn new(id: u64) -> Self {
        Self { id }
    }

    /// 返回账本内标识。
    pub const fn id(self) -> u64 {
        self.id
    }
}

struct FrameAccount {
    scope: u64,
    amount: u64,
    refs: u32,
    ledger: SharedLedger,
    charge: Charge,
    /// 完整覆盖一段物理区间时记录；部分重叠只登记未计费页。
    range: Option<PhysRange>,
    /// 本账目覆盖的物理页；无名记账为空。
    pages: Vec<u64>,
}

/// 进程运行时的统一 Frame 账本。
pub struct FrameLedger {
    next: u64,
    next_scope: u64,
    accounts: BTreeMap<u64, FrameAccount>,
    by_range: BTreeMap<(u64, u64, u64), u64>,
    by_page: BTreeMap<u64, Vec<u64>>,
    /// 尚未绑定记账的物理页映射引用数。
    mapping_refs: BTreeMap<u64, u32>,
}

impl FrameLedger {
    /// 构造空账本。
    pub const fn new() -> Self {
        Self {
            next: 1,
            next_scope: 1,
            accounts: BTreeMap::new(),
            by_range: BTreeMap::new(),
            by_page: BTreeMap::new(),
            mapping_refs: BTreeMap::new(),
        }
    }

    /// 记入一段无名常驻区间。
    pub fn charge(
        &mut self,
        scope: u64,
        ledger: &SharedLedger,
        amount: u64,
    ) -> Result<FrameRef, HmpError> {
        let charge = ledger.charge(amount)?;
        let id = self.insert(scope, amount, ledger.clone(), charge, None, Vec::new(), 1);
        Ok(FrameRef::new(id))
    }

    /// 返回指定作用域内覆盖该物理页的账目。
    fn account_in_scope(&self, scope: u64, phys: u64) -> Option<u64> {
        self.by_page.get(&phys).and_then(|ids| {
            ids.iter().copied().find(|id| {
                self.accounts
                    .get(id)
                    .is_some_and(|account| account.scope == scope)
            })
        })
    }

    /// 记入一段物理区间；同一作用域内同一页只扣一次。
    ///
    /// 完整重叠时复用既有账目并增加一次显式引用；部分重叠只为未计费页建立
    /// 新账目。这样同一任务映射同一物理页的多个别名不会重复计费，也不会
    /// 把另一个任务的账目合并进来。
    pub fn charge_range(
        &mut self,
        scope: u64,
        ledger: &SharedLedger,
        range: PhysRange,
    ) -> Result<FrameRef, HmpError> {
        if let Some(id) = self
            .by_range
            .get(&(scope, range.start().get(), range.len()))
            .copied()
        {
            self.retain_id(id);
            return Ok(FrameRef::new(id));
        }

        let page = hk_hal::PAGE_SIZE as u64;
        let pages: Vec<u64> = (range.start().get()..range.end().get())
            .step_by(page as usize)
            .collect();
        let mut uncovered = Vec::new();
        for phys in &pages {
            if self.account_in_scope(scope, *phys).is_none() {
                uncovered.push(*phys);
            }
        }
        if uncovered.is_empty() {
            // 范围已被本作用域完全覆盖。返回一个可释放的别名；该别名只
            // 持有首个既有账目的引用，其余账目仍由原映射持有。
            let id = pages
                .first()
                .and_then(|phys| self.account_in_scope(scope, *phys))
                .ok_or(HmpError::AlreadyMapped)?;
            self.retain_id(id);
            return Ok(FrameRef::new(id));
        }

        // 先扣账，成功后才修改任何索引；配额失败不会破坏既有映射。
        let amount = uncovered.len() as u64 * page;
        let charge = ledger.charge(amount)?;
        let full = uncovered.len() == pages.len();
        let id = self.insert(
            scope,
            amount,
            ledger.clone(),
            charge,
            full.then_some(range),
            uncovered.clone(),
            1,
        );
        if full {
            self.by_range
                .insert((scope, range.start().get(), range.len()), id);
        }
        for phys in uncovered {
            self.by_page.entry(phys).or_default().push(id);
        }
        Ok(FrameRef::new(id))
    }

    /// 接管一笔已经由内核/调用方扣过的记账。
    pub fn adopt(&mut self, scope: u64, ledger: SharedLedger, charge: Charge) -> FrameRef {
        let id = self.insert(scope, charge.amount(), ledger, charge, None, Vec::new(), 1);
        FrameRef::new(id)
    }

    #[allow(clippy::too_many_arguments)]
    fn insert(
        &mut self,
        scope: u64,
        amount: u64,
        ledger: SharedLedger,
        charge: Charge,
        range: Option<PhysRange>,
        pages: Vec<u64>,
        refs: u32,
    ) -> u64 {
        let id = self.next;
        self.next = self.next.saturating_add(1);
        self.accounts.insert(
            id,
            FrameAccount {
                scope,
                amount,
                refs,
                ledger,
                charge,
                range,
                pages,
            },
        );
        id
    }

    fn retain_id(&mut self, id: u64) {
        if let Some(account) = self.accounts.get_mut(&id) {
            account.refs = account.refs.saturating_add(1);
        }
    }

    /// 增加一次物理映射引用。
    pub fn acquire_mapping(&mut self, phys: u64) -> bool {
        self.mapping_refs
            .entry(phys)
            .and_modify(|count| *count = count.saturating_add(1))
            .or_insert(1);
        true
    }

    /// 释放一次物理映射引用。
    pub fn release_mapping(&mut self, phys: u64) -> u32 {
        let Some(count) = self.mapping_refs.get_mut(&phys) else {
            return 0;
        };
        *count = count.saturating_sub(1);
        if *count == 0 {
            self.mapping_refs.remove(&phys);
            0
        } else {
            *count
        }
    }

    /// 增加一次记账引用。
    pub fn retain(&mut self, frame: FrameRef) {
        self.retain_id(frame.id);
    }

    /// 释放一次记账引用。
    pub fn release(&mut self, frame: FrameRef) {
        let Some(account) = self.accounts.get_mut(&frame.id) else {
            return;
        };
        account.refs = account.refs.saturating_sub(1);
        if account.refs == 0 {
            self.release_account(frame.id);
        }
    }

    fn release_account(&mut self, id: u64) -> Option<FrameAccount> {
        let account = self.accounts.remove(&id)?;
        if let Some(range) = account.range {
            self.by_range
                .remove(&(account.scope, range.start().get(), range.len()));
        }
        for phys in &account.pages {
            if let Some(ids) = self.by_page.get_mut(phys) {
                ids.retain(|candidate| *candidate != id);
                if ids.is_empty() {
                    self.by_page.remove(phys);
                }
            }
        }
        let _ = account.ledger.release(account.charge);
        Some(account)
    }

    /// 返回引用记账字节数。
    pub fn amount(&self, frame: FrameRef) -> u64 {
        self.accounts
            .get(&frame.id)
            .map_or(0, |account| account.amount)
    }

    /// 返回当前跟踪的区间数。
    pub fn tracked(&self) -> usize {
        self.accounts.len()
    }
}

impl Default for FrameLedger {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::wildcard_imports)]
mod tests {
    use super::*;
    use hk_hal::PhysAddr;

    fn range(start: u64, len: u64) -> PhysRange {
        PhysRange::new(PhysAddr::new(start), len)
    }

    #[test]
    fn aliases_share_one_charge_until_last_release() {
        let ledger = SharedLedger::create(0x2000);
        let mut frames = FrameLedger::new();
        let first = frames
            .charge_range(1, &ledger, range(0x1000, 0x1000))
            .expect("first");
        let second = frames
            .charge_range(1, &ledger, range(0x1000, 0x1000))
            .expect("alias");
        assert_eq!(first, second);
        assert_eq!(ledger.charged(), 0x1000);
        frames.release(first);
        assert_eq!(ledger.charged(), 0x1000, "alias 仍持有");
        frames.release(second);
        assert_eq!(ledger.charged(), 0);
        assert_eq!(frames.tracked(), 0);
    }

    #[test]
    fn mapping_refs_are_independent_of_task_charges() {
        let ledger = SharedLedger::create(0x2000);
        let mut frames = FrameLedger::new();
        let frame = frames
            .charge_range(1, &ledger, range(0x2000, 0x1000))
            .expect("charge");
        assert!(frames.acquire_mapping(0x2000));
        assert!(frames.acquire_mapping(0x2000));
        assert_eq!(frames.release_mapping(0x2000), 1);
        frames.release(frame);
        assert_eq!(ledger.charged(), 0, "记账引用与映射引用解耦");
        assert_eq!(frames.release_mapping(0x2000), 0);
    }

    #[test]
    fn scopes_charge_shared_pages_separately() {
        let first = SharedLedger::create(0x2000);
        let second = SharedLedger::create(0x2000);
        let mut frames = FrameLedger::new();
        let first_ref = frames
            .charge_range(1, &first, range(0x3000, 0x1000))
            .expect("first");
        let second_ref = frames
            .charge_range(2, &second, range(0x3000, 0x1000))
            .expect("second");
        assert_ne!(first_ref, second_ref);
        assert_eq!(first.charged(), 0x1000);
        assert_eq!(second.charged(), 0x1000);
        frames.release(first_ref);
        assert_eq!(first.charged(), 0);
        assert_eq!(second.charged(), 0x1000);
        frames.release(second_ref);
        assert_eq!(second.charged(), 0);
    }

    #[test]
    fn partial_overlap_charges_only_uncovered_pages() {
        let ledger = SharedLedger::create(0x4000);
        let mut frames = FrameLedger::new();
        frames
            .charge_range(1, &ledger, range(0x5000, 0x1000))
            .expect("page a");
        frames
            .charge_range(1, &ledger, range(0x6000, 0x1000))
            .expect("page b");
        assert_eq!(ledger.charged(), 0x2000);
        assert_eq!(frames.tracked(), 2);
    }

    #[test]
    fn quota_failure_does_not_destroy_existing_charge() {
        let ledger = SharedLedger::create(0x1000);
        let mut frames = FrameLedger::new();
        let frame = frames
            .charge_range(1, &ledger, range(0x7000, 0x1000))
            .expect("first page");
        assert_eq!(
            frames.charge_range(1, &ledger, range(0x7000, 0x2000)).err(),
            Some(HmpError::LedgerLimit)
        );
        assert_eq!(ledger.charged(), 0x1000);
        frames.release(frame);
        assert_eq!(ledger.charged(), 0);
    }

    #[test]
    fn adopt_releases_preexisting_charge() {
        let ledger = SharedLedger::create(0x2000);
        let charge = ledger.charge(0x1000).expect("charge");
        let mut frames = FrameLedger::new();
        let frame = frames.adopt(1, ledger.clone(), charge);
        assert_eq!(ledger.charged(), 0x1000);
        frames.release(frame);
        assert_eq!(ledger.charged(), 0);
    }
}
