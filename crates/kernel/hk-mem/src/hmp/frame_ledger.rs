//! 统一 Frame 引用与任务记账。
//!
//! 一个 [`FrameRef`] 对应一份常驻区间：首次记入时从所属 Task Ledger 扣账，
//! 之后同一物理区间的别名只增加引用，不重复计费。最后一个引用释放时才归还
//! Task Ledger。非物理区间（如装载器虚拟段）也使用同一引用模型，只是没有
//! `range`。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use hk_hal::PhysRange;
use spin::Mutex;

use super::{Charge, HmpError, SharedLedger};

/// 全局物理帧账本：映射引用与任务记账共用同一对象。
static FRAME_LEDGER: Mutex<FrameLedger> = Mutex::new(FrameLedger::new());

/// 增加一次物理页映射引用。
pub fn mapping_acquire(phys: u64) -> bool {
    let mut ledger = FRAME_LEDGER.lock();
    if let Some(ids) = ledger.by_page.get(&phys).cloned() {
        for id in ids {
            if let Some(account) = ledger.accounts.get_mut(&id) {
                account.refs = account.refs.saturating_add(1);
            }
        }
    } else {
        ledger
            .refs
            .entry(phys)
            .and_modify(|count| *count += 1)
            .or_insert(1);
    }
    true
}

/// 释放一次物理页映射引用；返回剩余引用数。
pub fn mapping_release(phys: u64) -> u32 {
    FRAME_LEDGER.lock().release_mapping(phys)
}

/// 返回物理页映射引用数。
pub fn mapping_count(phys: u64) -> u32 {
    let ledger = FRAME_LEDGER.lock();
    if let Some(ids) = ledger.by_page.get(&phys) {
        return ids
            .iter()
            .filter_map(|id| ledger.accounts.get(id))
            .map(|account| account.refs)
            .max()
            .unwrap_or(0);
    }
    ledger.refs.get(&phys).copied().unwrap_or(0)
}

/// 清空全局映射引用；仅测试使用。
pub fn mapping_clear() {
    FRAME_LEDGER.lock().refs.clear();
}

/// 返回当前跟踪的物理页数。
pub fn mapping_tracked() -> usize {
    FRAME_LEDGER.lock().refs.len()
}

/// 分配一个记账作用域；每个运行时一个，避免不同运行时共享同一能力时
/// 把彼此的 Task Ledger 合并。
pub fn scope() -> u64 {
    let mut ledger = FRAME_LEDGER.lock();
    let scope = ledger.next_scope;
    ledger.next_scope = ledger.next_scope.saturating_add(1);
    scope
}

/// 记入一段常驻区间。
pub fn charge(scope: u64, ledger: &SharedLedger, amount: u64) -> Result<FrameRef, HmpError> {
    FRAME_LEDGER.lock().charge(scope, ledger, amount)
}

/// 记入一段物理区间。
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

/// 常驻区间引用。
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
    range: Option<PhysRange>,
    /// 物理页地址；`None` 表示无名记账区间。
    pages: Vec<u64>,
    /// 是否与平台映射引用共用 `refs`。
    mapped: bool,
}

/// 进程运行时的统一 Frame 账本。
pub struct FrameLedger {
    next: u64,
    next_scope: u64,
    accounts: BTreeMap<u64, FrameAccount>,
    by_range: BTreeMap<(u64, u64, u64), u64>,
    by_page: BTreeMap<u64, Vec<u64>>,
    /// 尚未绑定记账的物理页映射引用数；已绑定后由 account.refs 承接。
    refs: BTreeMap<u64, u32>,
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
            refs: BTreeMap::new(),
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
        let id = self.insert(
            scope,
            amount,
            ledger.clone(),
            charge,
            None,
            Vec::new(),
            false,
            1,
        );
        Ok(FrameRef::new(id))
    }

    /// 记入一段物理区间；同一作用域内已存在时只增加引用。
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
            return Ok(FrameRef::new(id));
        }
        let page = hk_hal::PAGE_SIZE as u64;
        let pages: Vec<u64> = (range.start().get()..range.end().get())
            .step_by(page as usize)
            .collect();
        let mapped = pages
            .iter()
            .map(|phys| self.refs.remove(phys).unwrap_or(0))
            .max()
            .unwrap_or(0);
        let charge = ledger.charge(range.len())?;
        let id = self.insert(
            scope,
            range.len(),
            ledger.clone(),
            charge,
            Some(range),
            pages.clone(),
            true,
            mapped,
        );
        self.by_range
            .insert((scope, range.start().get(), range.len()), id);
        for phys in pages {
            self.by_page.entry(phys).or_default().push(id);
        }
        Ok(FrameRef::new(id))
    }

    /// 接管一笔已经由内核/调用方扣过的记账。
    pub fn adopt(&mut self, scope: u64, ledger: SharedLedger, charge: Charge) -> FrameRef {
        let id = self.insert(
            scope,
            charge.amount(),
            ledger,
            charge,
            None,
            Vec::new(),
            false,
            1,
        );
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
        mapped: bool,
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
                mapped,
            },
        );
        id
    }

    /// 释放一次物理映射引用。
    pub fn release_mapping(&mut self, phys: u64) -> u32 {
        if let Some(ids) = self.by_page.get(&phys).cloned() {
            let mut remaining = 0;
            let mut release = Vec::new();
            for id in ids {
                let Some(account) = self.accounts.get_mut(&id) else {
                    continue;
                };
                account.refs = account.refs.saturating_sub(1);
                remaining = remaining.max(account.refs);
                if account.refs == 0 {
                    release.push(id);
                }
            }
            for id in release {
                self.release_account(id);
            }
            return remaining;
        }
        let Some(count) = self.refs.get_mut(&phys) else {
            return 0;
        };
        *count = count.saturating_sub(1);
        if *count == 0 {
            self.refs.remove(&phys);
            0
        } else {
            *count
        }
    }

    /// 增加一次引用。
    pub fn retain(&mut self, frame: FrameRef) {
        if let Some(account) = self.accounts.get_mut(&frame.id)
            && !account.mapped
        {
            account.refs = account.refs.saturating_add(1);
        }
    }

    /// 释放一次引用；映射模式由平台映射引用驱动。
    pub fn release(&mut self, frame: FrameRef) {
        let Some(account) = self.accounts.get_mut(&frame.id) else {
            return;
        };
        if account.mapped {
            if account.refs == 0 {
                self.release_account(frame.id);
            }
            return;
        }
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
mod tests {
    use super::*;
    use hk_hal::PhysAddr;

    #[test]
    fn aliases_share_one_charge_until_last_release() {
        let ledger = SharedLedger::create(0x2000);
        let mut frames = FrameLedger::new();
        let range = PhysRange::new(PhysAddr::new(0x1000), 0x1000);
        frames.refs.insert(range.start().get(), 2);
        let first = frames.charge_range(1, &ledger, range).expect("first");
        let second = frames.charge_range(1, &ledger, range).expect("alias");
        assert_eq!(first, second);
        assert_eq!(ledger.charged(), 0x1000);
        frames.release_mapping(range.start().get());
        assert_eq!(ledger.charged(), 0x1000);
        frames.release_mapping(range.start().get());
        assert_eq!(ledger.charged(), 0);
        assert_eq!(frames.tracked(), 0);
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
