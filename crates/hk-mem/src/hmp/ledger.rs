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
    charged: u64,
    peak: u64,
}

impl Ledger {
    /// 以配额创建 Ledger。
    pub const fn new(id: LedgerId, limit: u64) -> Self {
        Self {
            id,
            limit,
            charged: 0,
            peak: 0,
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

    /// 返回 Ledger 标识。
    pub const fn id(&self) -> LedgerId {
        self.id
    }

    /// 返回配额。
    pub const fn limit(&self) -> u64 {
        self.limit
    }

    /// 返回当前记账量。
    pub const fn charged(&self) -> u64 {
        self.charged
    }

    /// 返回峰值。
    pub const fn peak(&self) -> u64 {
        self.peak
    }

    /// 返回压力百分比（0–100）。
    pub const fn pressure(&self) -> u64 {
        if self.limit == 0 {
            return 100;
        }
        self.charged.saturating_mul(100) / self.limit
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
