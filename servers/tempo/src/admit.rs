//! Realtime 准入：请求解析与基于内核真实需求的 EDF 判定。
//!
//! Tempo 不再维护自己的契约账本；每次准入都通过 `SYS_CADENCE_DEMAND` 读取
//! 内核中存活 Realtime Task 的真实契约，再把候选契约加入需求界函数。这样
//! 任务退出、checkpoint 恢复与撤销都不需要额外的服务内回收动作。

use alloc::vec::Vec;

use hk_cadence::{CadenceClass, CadenceSnapshot, CadenceSpec, edf_admissible, micros};
use hk_proto::tempo::{MAX_NAME, SNAPSHOT_LEN};
use hk_user_api::{CADENCE_DEMAND_RECORD_BYTES, cadence_demand};

/// 内核当前 Realtime 需求的动态快照。
pub struct Demand {
    records: Vec<(u64, u64, u64)>,
}

impl Demand {
    /// 先查询记录数，再按需读取；`exclude` 为要跳过的 Cadence 标识。
    ///
    /// 返回 `None` 表示查询失败，调用方应按不可调度处理。
    pub fn load(exclude: u64) -> Option<Self> {
        let count = cadence_demand(exclude, &mut []);
        if count == u64::MAX {
            return None;
        }
        let count = usize::try_from(count).ok()?;
        let length = count.checked_mul(CADENCE_DEMAND_RECORD_BYTES)?;
        let mut bytes = alloc::vec![0_u8; length];
        let written = cadence_demand(exclude, &mut bytes);
        if written == u64::MAX {
            return None;
        }
        let written = usize::try_from(written).ok()?;
        if written > count {
            return None;
        }
        let mut records = Vec::with_capacity(written);
        for index in 0..written {
            let start = index * CADENCE_DEMAND_RECORD_BYTES;
            records.push((
                word(&bytes, start),
                word(&bytes, start + 8),
                word(&bytes, start + 16),
            ));
        }
        Some(Self { records })
    }

    /// 判定加入候选契约后整组 Realtime 任务仍可由 EDF 调度。
    pub fn allows(&self, budget: u64, period: u64, deadline: u64) -> bool {
        let mut tasks = Vec::with_capacity(self.records.len() + 1);
        tasks.extend_from_slice(&self.records);
        tasks.push((budget, period, deadline));
        edf_admissible(&tasks)
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
    pool_latency: u64,
    exclude: u64,
) -> Option<CadenceSpec> {
    if snapshot.period != 0 && snapshot.period != pool_period {
        return None;
    }
    if snapshot.budget > pool_budget {
        return None;
    }
    let effective_pool_deadline = if pool_deadline == 0 {
        pool_period
    } else {
        pool_deadline
    };
    let requested_deadline = if snapshot.deadline == 0 {
        effective_pool_deadline
    } else {
        snapshot.deadline.min(effective_pool_deadline)
    };
    if requested_deadline == 0 {
        return None;
    }
    let mut spec = snapshot.spec();
    spec.class = CadenceClass::Realtime;
    spec.budget = micros(snapshot.budget);
    spec.period = micros(pool_period);
    spec.deadline = micros(requested_deadline);
    spec.latency = micros(snapshot.latency.min(pool_latency));
    let demand = Demand::load(exclude)?;
    demand
        .allows(
            spec.budget.ticks(),
            spec.period.ticks(),
            spec.deadline.ticks(),
        )
        .then_some(spec)
}

/// 把策略方准入后的契约编码为 `SYS_CADENCE_SPLIT` 请求快照。
pub fn encode_spec(spec: CadenceSpec) -> [u8; CadenceSnapshot::ENCODED_LEN] {
    let mut bytes = [0_u8; CadenceSnapshot::ENCODED_LEN];
    CadenceSnapshot::from_spec(spec).encode(&mut bytes);
    bytes
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
