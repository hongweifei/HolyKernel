//! Tempo：调度策略服务。
//!
//! Root Server 授予 Tempo 一个带硬预算的初始 Cadence（策略池）。Tempo 在池内
//! 派生收窄的子契约，并把新契约句柄随回复转移给调用方；调用方随后用
//! `SYS_TASK_ATTACH_CADENCE(task = 0, cadence)` 附加到自己。
//!
//! 本 crate 只在 `x86_64-unknown-none` 目标上构建；宿主构建保留占位入口，便于
//! `cargo clippy --workspace --all-targets` 与工作区测试。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use hk_cadence::{CadenceClass, CadenceSnapshot, CadenceSpec, micros};
#[cfg(target_os = "none")]
use hk_proto::tempo::{
    MAX_NAME, REJECT_INVALID, REJECT_OUT_OF_RANGE, SNAPSHOT_LEN, TEMPO_ADMIT, TEMPO_READMIT,
    TEMPO_REJECTED, TEMPO_REQUEST, TEMPO_RESPONSE, TEMPO_REVOKE,
};
#[cfg(target_os = "none")]
use hk_server_api::{
    RIGHT_READ, RIGHT_TRANSFER, RIGHT_WRITE, cadence_named, cadence_self, cadence_self_handle,
    cadence_split, close_handle, exit, handle_duplicate, recv_data, send, send_handle,
    task_attach_named, task_regrant_named, write,
};

/// 请求缓冲区：容纳按名请求的名字前缀与 Cadence 快照。
#[cfg(target_os = "none")]
const MAX_REQUEST: usize = 2 + MAX_NAME + CadenceSnapshot::ENCODED_LEN;

/// 准入账本容量。
#[cfg(target_os = "none")]
const MAX_CONTRACTS: usize = 16;

/// Realtime 准入账本；记录按名契约与匿名申请。
#[cfg(target_os = "none")]
struct Ledger {
    names: [[u8; MAX_NAME]; MAX_CONTRACTS],
    lengths: [u8; MAX_CONTRACTS],
    used: [bool; MAX_CONTRACTS],
    budgets: [u64; MAX_CONTRACTS],
    periods: [u64; MAX_CONTRACTS],
    deadlines: [u64; MAX_CONTRACTS],
}

#[cfg(target_os = "none")]
impl Ledger {
    const fn new() -> Self {
        Self {
            names: [[0; MAX_NAME]; MAX_CONTRACTS],
            lengths: [0; MAX_CONTRACTS],
            used: [false; MAX_CONTRACTS],
            budgets: [0; MAX_CONTRACTS],
            periods: [0; MAX_CONTRACTS],
            deadlines: [0; MAX_CONTRACTS],
        }
    }

    /// 把启动清单中未经过 Tempo 的 Realtime 服务计入基线负载。
    fn seed_base_load(&mut self) {
        for service in hk_catalog::SERVICES {
            if !service.boot
                || service.name == "hk-server-tempo"
                || !hk_catalog::is_realtime(service.name)
            {
                continue;
            }
            let Some(slot) = self.free() else {
                break;
            };
            self.used[slot] = true;
            self.budgets[slot] = service.cadence.budget.ticks();
            self.periods[slot] = service.cadence.period.ticks();
            self.deadlines[slot] = service.cadence.deadline.ticks();
        }
    }

    fn find(&self, name: &str) -> Option<usize> {
        let name = name.as_bytes();
        self.names.iter().enumerate().find_map(|(index, entry)| {
            let length = usize::from(self.lengths[index]);
            (self.used[index] && length == name.len() && &entry[..length] == name).then_some(index)
        })
    }

    fn free(&self) -> Option<usize> {
        self.used.iter().position(|used| !*used)
    }

    /// 以 demand bound function 判定加入/替换一个契约后是否可调度。
    fn admissible_with(
        &self,
        replace: Option<usize>,
        budget: u64,
        period: u64,
        deadline: u64,
    ) -> bool {
        let mut tasks = [(0_u64, 0_u64, 0_u64); MAX_CONTRACTS + 1];
        let mut count = 0;
        for index in 0..MAX_CONTRACTS {
            if !self.used[index] || Some(index) == replace {
                continue;
            }
            tasks[count] = (
                self.budgets[index],
                self.periods[index],
                self.deadlines[index],
            );
            count += 1;
        }
        tasks[count] = (budget, period, deadline);
        hk_cadence::edf_admissible(&tasks[..=count])
    }

    fn admit_named(&mut self, name: &str, budget: u64, period: u64, deadline: u64) -> bool {
        let existing = self.find(name);
        let slot = match existing {
            Some(index) => index,
            None => {
                let Some(index) = self.free() else {
                    return false;
                };
                index
            }
        };
        if !self.admissible_with(existing, budget, period, deadline) {
            return false;
        }
        if existing.is_none() {
            let length = name.len();
            self.names[slot][..length].copy_from_slice(name.as_bytes());
            self.lengths[slot] = length as u8;
        }
        self.used[slot] = true;
        self.budgets[slot] = budget;
        self.periods[slot] = period;
        self.deadlines[slot] = deadline;
        true
    }

    fn admit_unnamed(&mut self, budget: u64, period: u64, deadline: u64) -> bool {
        let Some(slot) = self.free() else {
            return false;
        };
        if !self.admissible_with(None, budget, period, deadline) {
            return false;
        }
        self.used[slot] = true;
        self.lengths[slot] = 0;
        self.budgets[slot] = budget;
        self.periods[slot] = period;
        self.deadlines[slot] = deadline;
        true
    }
}

/// 按池的周期与预算构造准入检查用的契约。
#[cfg(target_os = "none")]
fn admission_spec(
    snapshot: &CadenceSnapshot,
    pool_budget: u64,
    pool_period: u64,
    pool_deadline: u64,
) -> CadenceSpec {
    let mut spec = snapshot.spec();
    spec.class = CadenceClass::Realtime;
    spec.budget = micros(snapshot.budget.min(pool_budget));
    spec.period = micros(pool_period);
    spec.deadline = micros(if pool_deadline == 0 {
        pool_period
    } else {
        pool_deadline
    });
    spec
}

/// 解析只有名字的请求。
#[cfg(target_os = "none")]
fn name_request(bytes: &[u8]) -> Option<&str> {
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
#[cfg(target_os = "none")]
fn named_request(bytes: &[u8]) -> Option<(&str, &[u8])> {
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

/// 服务入口：内核以用户态进入，参数是内核分配给本服务的端点编号。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let pool = cadence_self_handle();
    if pool == u64::MAX {
        write(b"hk-server-tempo: cadence handle fail\n");
        exit(1);
    }
    let mut pool_bytes = [0_u8; CadenceSnapshot::ENCODED_LEN];
    if cadence_self(&mut pool_bytes) != CadenceSnapshot::ENCODED_LEN as u64 {
        write(b"hk-server-tempo: pool info fail\n");
        exit(1);
    }
    let Some(pool_snapshot) = CadenceSnapshot::decode(&pool_bytes).ok() else {
        write(b"hk-server-tempo: pool decode fail\n");
        exit(1);
    };
    let mut ledger = Ledger::new();
    ledger.seed_base_load();
    write(b"hk-server-tempo: ready\n");

    let mut buffer = [0_u8; MAX_REQUEST];
    loop {
        let (label, _word, sender, size) = recv_data(endpoint, &mut buffer);
        if label == TEMPO_READMIT {
            let Some(name) = name_request(&buffer[..size]) else {
                let _ = send(sender, TEMPO_REJECTED, REJECT_INVALID);
                continue;
            };
            let mut snapshot = [0_u8; CadenceSnapshot::ENCODED_LEN];
            if cadence_named(name, &mut snapshot) != CadenceSnapshot::ENCODED_LEN as u64 {
                let _ = send(sender, TEMPO_REJECTED, REJECT_OUT_OF_RANGE);
                continue;
            }
            let Some(request) = CadenceSnapshot::decode(&snapshot).ok() else {
                let _ = send(sender, TEMPO_REJECTED, REJECT_INVALID);
                continue;
            };
            let spec = admission_spec(
                &request,
                pool_snapshot.budget,
                pool_snapshot.period,
                pool_snapshot.deadline,
            );
            if !ledger.admit_named(
                name,
                spec.budget.ticks(),
                spec.period.ticks(),
                spec.deadline.ticks(),
            ) {
                let _ = send(sender, TEMPO_REJECTED, REJECT_OUT_OF_RANGE);
                continue;
            }
            let child = cadence_split(pool, &snapshot);
            if child == u64::MAX {
                let _ = send(sender, TEMPO_REJECTED, REJECT_OUT_OF_RANGE);
                continue;
            }
            let result = task_regrant_named(name, child);
            let _ = close_handle(child);
            if result == 0 {
                let _ = send(sender, TEMPO_RESPONSE, 0);
            } else {
                let _ = send(sender, TEMPO_REJECTED, REJECT_OUT_OF_RANGE);
            }
            continue;
        }
        if label == TEMPO_ADMIT || label == TEMPO_REVOKE {
            let Some((name, snapshot)) = named_request(&buffer[..size]) else {
                let _ = send(sender, TEMPO_REJECTED, REJECT_INVALID);
                continue;
            };
            let Some(request) = CadenceSnapshot::decode(snapshot).ok() else {
                let _ = send(sender, TEMPO_REJECTED, REJECT_INVALID);
                continue;
            };
            let spec = admission_spec(
                &request,
                pool_snapshot.budget,
                pool_snapshot.period,
                pool_snapshot.deadline,
            );
            if !ledger.admit_named(
                name,
                spec.budget.ticks(),
                spec.period.ticks(),
                spec.deadline.ticks(),
            ) {
                let _ = send(sender, TEMPO_REJECTED, REJECT_OUT_OF_RANGE);
                continue;
            }
            let child = cadence_split(pool, snapshot);
            if child == u64::MAX {
                let _ = send(sender, TEMPO_REJECTED, REJECT_OUT_OF_RANGE);
                continue;
            }
            let result = if label == TEMPO_ADMIT {
                task_attach_named(name, child)
            } else {
                task_regrant_named(name, child)
            };
            let _ = close_handle(child);
            if result == 0 {
                let _ = send(sender, TEMPO_RESPONSE, 0);
            } else {
                let _ = send(sender, TEMPO_REJECTED, REJECT_OUT_OF_RANGE);
            }
            continue;
        }
        if label != TEMPO_REQUEST {
            continue;
        }
        if size < CadenceSnapshot::ENCODED_LEN {
            let _ = send(sender, TEMPO_REJECTED, REJECT_INVALID);
            continue;
        }
        let Some(request) = CadenceSnapshot::decode(&buffer[..CadenceSnapshot::ENCODED_LEN]).ok()
        else {
            let _ = send(sender, TEMPO_REJECTED, REJECT_INVALID);
            continue;
        };
        let spec = admission_spec(
            &request,
            pool_snapshot.budget,
            pool_snapshot.period,
            pool_snapshot.deadline,
        );
        if !ledger.admit_unnamed(
            spec.budget.ticks(),
            spec.period.ticks(),
            spec.deadline.ticks(),
        ) {
            let _ = send(sender, TEMPO_REJECTED, REJECT_OUT_OF_RANGE);
            continue;
        }
        let child = cadence_split(pool, &buffer[..CadenceSnapshot::ENCODED_LEN]);
        if child == u64::MAX {
            let _ = send(sender, TEMPO_REJECTED, REJECT_OUT_OF_RANGE);
            continue;
        }
        let granted = handle_duplicate(child, RIGHT_READ | RIGHT_WRITE | RIGHT_TRANSFER);
        let _ = close_handle(child);
        if granted == u64::MAX {
            let _ = send(sender, TEMPO_REJECTED, REJECT_OUT_OF_RANGE);
            continue;
        }
        if send_handle(sender, TEMPO_RESPONSE, 0, granted) != 0 {
            let _ = send(sender, TEMPO_REJECTED, REJECT_OUT_OF_RANGE);
        }
    }
}

hk_server_api::server_boilerplate!();
