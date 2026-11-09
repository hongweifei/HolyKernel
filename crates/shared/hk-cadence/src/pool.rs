//! Cadence 预算池。
//!
//! Root Server 持有初始预算池，Tempo Server 从中为契约分配预算；每次分配返回
//! 租约，释放时归还。预算池只做记账，不决定策略。

use core::sync::atomic::{AtomicU64, Ordering};

use hk_base::Duration;

use crate::CadenceError;

static NEXT_POOL_ID: AtomicU64 = AtomicU64::new(1);

/// 预算池标识。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BudgetPoolId(u64);

impl BudgetPoolId {
    /// 以数值构造标识。
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// 分配一个进程内唯一标识。
    pub fn next() -> Self {
        Self(NEXT_POOL_ID.fetch_add(1, Ordering::Relaxed))
    }

    /// 返回标识数值。
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// 一次预算租约。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BudgetLease {
    pool: BudgetPoolId,
    amount: Duration,
}

impl BudgetLease {
    /// 返回所属预算池。
    pub const fn pool(self) -> BudgetPoolId {
        self.pool
    }

    /// 返回租约预算。
    pub const fn amount(self) -> Duration {
        self.amount
    }
}

/// 预算池。
pub struct BudgetPool {
    id: BudgetPoolId,
    limit: Duration,
    allocated: Duration,
}

impl BudgetPool {
    /// 以限额构造预算池。
    pub fn new(limit: Duration) -> Self {
        Self::with_id(BudgetPoolId::next(), limit)
    }

    /// 以指定标识与限额构造预算池，用于确定性回放与测试。
    pub const fn with_id(id: BudgetPoolId, limit: Duration) -> Self {
        Self {
            id,
            limit,
            allocated: Duration::ZERO,
        }
    }

    /// 分配预算。
    pub fn allocate(&mut self, amount: Duration) -> Result<BudgetLease, CadenceError> {
        if amount.is_zero() {
            return Err(CadenceError::ZeroBudget);
        }
        let next = self
            .allocated
            .ticks()
            .checked_add(amount.ticks())
            .ok_or(CadenceError::BudgetPoolExhausted)?;
        if next > self.limit.ticks() {
            return Err(CadenceError::BudgetPoolExhausted);
        }
        self.allocated = Duration::from_ticks(next);
        Ok(BudgetLease {
            pool: self.id,
            amount,
        })
    }

    /// 归还预算；租约必须属于该池。
    pub fn release(&mut self, lease: BudgetLease) -> Result<(), CadenceError> {
        if lease.pool != self.id {
            return Err(CadenceError::BudgetPoolForeign);
        }
        self.allocated =
            Duration::from_ticks(self.allocated.ticks().saturating_sub(lease.amount.ticks()));
        Ok(())
    }

    /// 返回预算池标识。
    pub const fn id(&self) -> BudgetPoolId {
        self.id
    }

    /// 返回限额。
    pub const fn limit(&self) -> Duration {
        self.limit
    }

    /// 返回已分配预算。
    pub const fn allocated(&self) -> Duration {
        self.allocated
    }

    /// 返回剩余预算。
    pub const fn available(&self) -> Duration {
        Duration::from_ticks(self.limit.ticks().saturating_sub(self.allocated.ticks()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_releases_and_exhausts() {
        let mut pool = BudgetPool::with_id(BudgetPoolId::new(1), Duration::from_ticks(10));
        let first = pool.allocate(Duration::from_ticks(4)).expect("第一次分配");
        let second = pool.allocate(Duration::from_ticks(6)).expect("第二次分配");
        assert_eq!(pool.allocated(), Duration::from_ticks(10));
        assert_eq!(pool.available(), Duration::ZERO);
        assert_eq!(
            pool.allocate(Duration::from_ticks(1)).err(),
            Some(CadenceError::BudgetPoolExhausted)
        );

        pool.release(first).expect("归还");
        assert_eq!(pool.allocated(), Duration::from_ticks(6));
        pool.release(second).expect("归还");
        assert_eq!(pool.allocated(), Duration::ZERO);
    }

    #[test]
    fn rejects_foreign_zero_and_overflow() {
        let mut pool = BudgetPool::with_id(BudgetPoolId::new(1), Duration::from_ticks(10));
        let mut other = BudgetPool::with_id(BudgetPoolId::new(2), Duration::from_ticks(10));
        let lease = pool.allocate(Duration::from_ticks(1)).expect("分配");
        assert_eq!(
            other.release(lease).err(),
            Some(CadenceError::BudgetPoolForeign)
        );
        assert_eq!(
            pool.allocate(Duration::ZERO).err(),
            Some(CadenceError::ZeroBudget)
        );
        assert_eq!(
            pool.allocate(Duration::from_ticks(u64::MAX)).err(),
            Some(CadenceError::BudgetPoolExhausted)
        );
    }
}
