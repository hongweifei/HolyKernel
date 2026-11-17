//! 统一 Frame 引用与任务记账。
//!
//! 一个 [`FrameRef`] 对应一份常驻区间：首次记入时从所属 Task Ledger 扣账，
//! 之后同一物理区间的别名只增加引用，不重复计费。最后一个引用释放时才归还
//! Task Ledger。非物理区间（如装载器虚拟段）也使用同一引用模型，只是没有
//! `range`。

use alloc::collections::BTreeMap;

use hk_hal::PhysRange;
use spin::Mutex;

use super::{Charge, HmpError, SharedLedger};

/// 全局物理帧账本：映射引用与任务记账共用同一对象。
static FRAME_LEDGER: Mutex<FrameLedger> = Mutex::new(FrameLedger::new());

/// 增加一次物理页映射引用。
pub fn mapping_acquire(phys: u64) -> bool {
    let mut ledger = FRAME_LEDGER.lock();
    ledger
        .refs
        .entry(phys)
        .and_modify(|count| *count += 1)
        .or_insert(1);
    true
}

/// 释放一次物理页映射引用；返回剩余引用数。
pub fn mapping_release(phys: u64) -> u32 {
    let mut ledger = FRAME_LEDGER.lock();
    let Some(count) = ledger.refs.get_mut(&phys) else {
        return 0;
    };
    *count = count.saturating_sub(1);
    if *count == 0 {
        ledger.refs.remove(&phys);
        0
    } else {
        *count
    }
}

/// 返回物理页映射引用数。
pub fn mapping_count(phys: u64) -> u32 {
    FRAME_LEDGER.lock().refs.get(&phys).copied().unwrap_or(0)
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
}

/// 进程运行时的统一 Frame 账本。
pub struct FrameLedger {
    next: u64,
    next_scope: u64,
    accounts: BTreeMap<u64, FrameAccount>,
    by_range: BTreeMap<(u64, u64, u64), u64>,
    /// 物理页映射引用数；与记账同属一个账本，但生命周期独立。
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
        let id = self.insert(scope, amount, ledger.clone(), charge, None);
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
            self.retain(FrameRef::new(id));
            return Ok(FrameRef::new(id));
        }
        let charge = ledger.charge(range.len())?;
        let id = self.insert(scope, range.len(), ledger.clone(), charge, Some(range));
        self.by_range
            .insert((scope, range.start().get(), range.len()), id);
        Ok(FrameRef::new(id))
    }

    /// 接管一笔已经由内核/调用方扣过的记账。
    pub fn adopt(&mut self, scope: u64, ledger: SharedLedger, charge: Charge) -> FrameRef {
        let id = self.insert(scope, charge.amount(), ledger, charge, None);
        FrameRef::new(id)
    }

    fn insert(
        &mut self,
        scope: u64,
        amount: u64,
        ledger: SharedLedger,
        charge: Charge,
        range: Option<PhysRange>,
    ) -> u64 {
        let id = self.next;
        self.next = self.next.saturating_add(1);
        self.accounts.insert(
            id,
            FrameAccount {
                scope,
                amount,
                refs: 1,
                ledger,
                charge,
                range,
            },
        );
        id
    }

    /// 增加一次引用。
    pub fn retain(&mut self, frame: FrameRef) {
        if let Some(account) = self.accounts.get_mut(&frame.id) {
            account.refs = account.refs.saturating_add(1);
        }
    }

    /// 释放一次引用；最后一个引用释放时归还 Task Ledger。
    pub fn release(&mut self, frame: FrameRef) {
        let Some(account) = self.accounts.get_mut(&frame.id) else {
            return;
        };
        account.refs = account.refs.saturating_sub(1);
        if account.refs != 0 {
            return;
        }
        let account = self
            .accounts
            .remove(&frame.id)
            .expect("account checked above");
        if let Some(range) = account.range {
            self.by_range
                .remove(&(account.scope, range.start().get(), range.len()));
        }
        let _ = account.ledger.release(account.charge);
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
        let first = frames.charge_range(1, &ledger, range).expect("first");
        let second = frames.charge_range(1, &ledger, range).expect("alias");
        assert_eq!(first, second);
        assert_eq!(ledger.charged(), 0x1000);
        frames.release(first);
        assert_eq!(ledger.charged(), 0x1000);
        frames.release(second);
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
