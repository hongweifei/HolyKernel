//! Ledger：容量、常驻、配额与压力记账。

use core::sync::atomic::{AtomicU64, Ordering};

use super::HmpError;

static NEXT_LEDGER_ID: AtomicU64 = AtomicU64::new(1);

/// Ledger 标识。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LedgerId(u64);

impl LedgerId {
    /// 构造标识。
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// 返回标识数值。
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Ledger 只读快照。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LedgerSnapshot {
    /// 当前已记账字节数。
    pub charged: u64,
    /// 硬上限。
    pub limit: u64,
    /// 峰值。
    pub peak: u64,
    /// 压力百分比。
    pub pressure: u64,
    /// 常驻字节数。
    pub resident: u64,
    /// 软上限。
    pub soft_limit: u64,
}

impl LedgerSnapshot {
    /// 由对象当前占用与容量构造报告快照。
    ///
    /// 该快照只描述利用率，不参与硬配额判断；硬配额由 Task Ledger 与
    /// Capacity/FrameBank 边界执行。
    pub fn utilization(charged: u64, limit: u64, peak: u64, resident: u64) -> Self {
        Self {
            charged,
            limit,
            peak,
            pressure: charged
                .saturating_mul(100)
                .checked_div(limit)
                .unwrap_or(100),
            resident,
            soft_limit: 0,
        }
    }
}

/// 一次记账。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Charge {
    ledger: LedgerId,
    amount: u64,
}

impl Charge {
    /// 返回记账所属 Ledger。
    pub const fn ledger(self) -> LedgerId {
        self.ledger
    }

    /// 返回记账字节数。
    pub const fn amount(self) -> u64 {
        self.amount
    }
}

/// 显式记账对象。
pub struct Ledger {
    id: LedgerId,
    limit: u64,
    soft_limit: u64,
    charged: u64,
    peak: u64,
    resident: u64,
}

impl Ledger {
    /// 以配额创建 Ledger。
    pub const fn new(id: LedgerId, limit: u64) -> Self {
        Self {
            id,
            limit,
            soft_limit: 0,
            charged: 0,
            peak: 0,
            resident: 0,
        }
    }

    /// 分配一个 Ledger。
    pub fn create(limit: u64) -> Self {
        Self::new(
            LedgerId::new(NEXT_LEDGER_ID.fetch_add(1, Ordering::Relaxed)),
            limit,
        )
    }

    /// 记入一段容量。
    pub fn charge(&mut self, amount: u64) -> Result<Charge, HmpError> {
        let next = self
            .charged
            .checked_add(amount)
            .ok_or(HmpError::LedgerLimit)?;
        if next > self.limit {
            return Err(HmpError::LedgerLimit);
        }
        self.charged = next;
        self.peak = self.peak.max(next);
        Ok(Charge {
            ledger: self.id,
            amount,
        })
    }

    /// 释放一次记账。
    pub fn release(&mut self, charge: Charge) -> Result<(), HmpError> {
        if charge.ledger != self.id {
            return Err(HmpError::WrongCapability);
        }
        self.charged = self.charged.saturating_sub(charge.amount);
        Ok(())
    }

    /// 按字节数释放记账；用于对象内部以固定页大小维护的账本。
    pub fn release_amount(&mut self, amount: u64) -> Result<(), HmpError> {
        self.charged = self.charged.saturating_sub(amount);
        Ok(())
    }

    /// 返回 Ledger 标识。
    pub const fn id(&self) -> LedgerId {
        self.id
    }

    /// 返回配额。
    pub const fn limit(&self) -> u64 {
        self.limit
    }

    /// 更新硬上限；软上限与已记账量保持不变。
    pub fn set_limit(&mut self, limit: u64) {
        self.limit = limit;
    }

    /// 返回当前记账量。
    pub const fn charged(&self) -> u64 {
        self.charged
    }

    /// 返回峰值。
    pub const fn peak(&self) -> u64 {
        self.peak
    }

    /// 设置软上限；0 表示未设置。软上限只用于报告，不拒绝分配。
    pub fn set_soft_limit(&mut self, limit: u64) {
        self.soft_limit = limit;
    }

    /// 返回软上限。
    pub const fn soft_limit(&self) -> u64 {
        self.soft_limit
    }

    /// 更新常驻字节数快照。
    ///
    /// 常驻由运行时按能力后备与已装入页计算；这里只保存报告值，不参与硬配额
    /// 判断（硬配额看 `charged`）。
    pub fn set_resident(&mut self, resident: u64) {
        self.resident = resident;
    }

    /// 返回常驻字节数快照。
    pub const fn resident(&self) -> u64 {
        self.resident
    }

    /// 返回压力百分比（0–100）。
    pub const fn pressure(&self) -> u64 {
        if self.limit == 0 {
            return 100;
        }
        self.charged.saturating_mul(100) / self.limit
    }

    /// 返回只读快照。
    pub const fn snapshot(&self) -> LedgerSnapshot {
        LedgerSnapshot {
            charged: self.charged,
            limit: self.limit,
            peak: self.peak,
            pressure: self.pressure(),
            resident: self.resident,
            soft_limit: self.soft_limit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charges_releases_and_tracks_peak() {
        let mut ledger = Ledger::new(LedgerId::new(1), 0x3000);
        let first = ledger.charge(0x1000).expect("第一次");
        let _second = ledger.charge(0x2000).expect("第二次");
        assert_eq!(ledger.charged(), 0x3000);
        assert_eq!(ledger.peak(), 0x3000);
        assert_eq!(ledger.pressure(), 100);

        ledger.release(first).expect("释放");
        assert_eq!(ledger.charged(), 0x2000);
        assert_eq!(ledger.peak(), 0x3000);
    }

    #[test]
    fn reports_soft_limit_and_resident() {
        let mut ledger = Ledger::new(LedgerId::new(3), 0x4000);
        ledger.set_soft_limit(0x3000);
        ledger.set_resident(0x1800);
        assert_eq!(ledger.soft_limit(), 0x3000);
        assert_eq!(ledger.resident(), 0x1800);
        assert_eq!(ledger.pressure(), 0);
        ledger.charge(0x2000).expect("记账");
        assert_eq!(ledger.pressure(), 50);
    }

    #[test]
    fn rejects_over_limit_and_foreign_charge() {
        let mut first = Ledger::new(LedgerId::new(1), 0x1000);
        let mut second = Ledger::new(LedgerId::new(2), 0x1000);
        let charge = first.charge(0x1000).expect("记账");
        assert_eq!(first.charge(1).err(), Some(HmpError::LedgerLimit));
        assert_eq!(
            second.release(charge).err(),
            Some(HmpError::WrongCapability)
        );
    }
}
