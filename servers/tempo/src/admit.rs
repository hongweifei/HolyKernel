//! Realtime 准入：请求解析与基于内核真实需求的 EDF 判定。
//!
//! Tempo 不再维护自己的契约账本；每次准入都通过 `SYS_CADENCE_DEMAND` 读取
//! 内核中存活 Realtime Task 的真实契约，再把候选契约加入需求界函数。这样
//! 任务退出、checkpoint 恢复与撤销都不需要额外的服务内回收动作。

use hk_cadence::{CadenceClass, CadenceSnapshot, CadenceSpec, edf_admissible, micros};
use hk_proto::tempo::{MAX_NAME, SNAPSHOT_LEN};
use hk_user_api::{CADENCE_DEMAND_RECORD_BYTES, cadence_demand};

/// 一次需求查询最多容纳的契约数。
pub const DEMAND_RECORDS: usize = 32;

/// 需求查询缓冲区长度。
pub const DEMAND_BYTES: usize = DEMAND_RECORDS * CADENCE_DEMAND_RECORD_BYTES;

/// 内核当前 Realtime 需求的定长快照。
pub struct Demand {
    records: [(u64, u64, u64); DEMAND_RECORDS],
    count: usize,
}

impl Demand {
    /// 读取需求；`exclude` 为要跳过的 Cadence 标识。
    ///
    /// 返回 `None` 表示查询失败或记录数超过固定容量，调用方应按不可调度处理。
    pub fn load(exclude: u64) -> Option<Self> {
        let mut bytes = [0_u8; DEMAND_BYTES];
        let count = cadence_demand(exclude, &mut bytes);
        if count == u64::MAX || count as usize > DEMAND_RECORDS {
            return None;
        }
        let mut records = [(0_u64, 0_u64, 0_u64); DEMAND_RECORDS];
        for (index, record) in records.iter_mut().take(count as usize).enumerate() {
            let start = index * CADENCE_DEMAND_RECORD_BYTES;
            *record = (
                word(&bytes, start),
                word(&bytes, start + 8),
                word(&bytes, start + 16),
            );
        }
        Some(Self {
            records,
            count: count as usize,
        })
    }

    /// 判定加入候选契约后整组 Realtime 任务仍可由 EDF 调度。
    pub fn allows(&self, budget: u64, period: u64, deadline: u64) -> bool {
        let mut tasks = [(0_u64, 0_u64, 0_u64); DEMAND_RECORDS + 1];
        tasks[..self.count].copy_from_slice(&self.records[..self.count]);
        tasks[self.count] = (budget, period, deadline);
        edf_admissible(&tasks[..=self.count])
    }
}

fn word(bytes: &[u8], start: usize) -> u64 {
    u64::from_le_bytes(
        bytes[start..start + 8]
            .try_into()
            .expect("需求记录按定长切分"),
    )
}

/// 把请求快照收窄到策略池范围内，并按 Realtime 契约做准入。
pub fn admission_spec(
    snapshot: &CadenceSnapshot,
    pool_budget: u64,
    pool_period: u64,
    pool_deadline: u64,
    exclude: u64,
) -> Option<CadenceSpec> {
    let mut spec = snapshot.spec();
    spec.class = CadenceClass::Realtime;
    spec.budget = micros(snapshot.budget.min(pool_budget));
    spec.period = micros(pool_period);
    spec.deadline = micros(if pool_deadline == 0 {
        pool_period
    } else {
        pool_deadline
    });
    let demand = Demand::load(exclude)?;
    demand
        .allows(
            spec.budget.ticks(),
            spec.period.ticks(),
            spec.deadline.ticks(),
        )
        .then_some(spec)
}

/// 解析只有名字的请求。
pub fn name_request(bytes: &[u8]) -> Option<&str> {
    if bytes.len() < 2 {
        return None;
    }
    let length = u16::from_le_bytes([bytes[0], bytes[1]]) as usize;
    if length == 0 || length > MAX_NAME || bytes.len() < 2 + length {
        return None;
    }
    core::str::from_utf8(&bytes[2..2 + length]).ok()
}

/// 解析按名请求；返回任务名与快照。
pub fn named_request(bytes: &[u8]) -> Option<(&str, &[u8])> {
    if bytes.len() < 2 + SNAPSHOT_LEN {
        return None;
    }
    let length = u16::from_le_bytes([bytes[0], bytes[1]]) as usize;
    if length == 0 || length > MAX_NAME || bytes.len() < 2 + length + SNAPSHOT_LEN {
        return None;
    }
    let name = core::str::from_utf8(&bytes[2..2 + length]).ok()?;
    let snapshot = &bytes[2 + length..2 + length + SNAPSHOT_LEN];
    CadenceSnapshot::decode(snapshot).ok()?;
    Some((name, snapshot))
}
