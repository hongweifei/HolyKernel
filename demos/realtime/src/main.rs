//! Realtime 硬截止期探针。
//!
//! 通过 Tempo 准入一个 Realtime 契约，运行若干个周期后读取 Meter，要求
//! 没有任何 dispatch 超过绝对截止期，并确认预算节流实际发生。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use hk_cadence::{CadenceClass, CadenceSnapshot, CpuMask, Lending, MeterSnapshot, millis};
#[cfg(target_os = "none")]
use hk_proto_client::TempoClient;
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::{
    cadence_self, exit, meter_info, runtime::wait_service, task_attach_cadence, ticks, tsc, write,
    write_u64,
};

/// 服务堆大小。
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 64 * 1024;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

/// 观测窗口（调度 tick，10 ms）。
#[cfg(target_os = "none")]
const WINDOW_TICKS: u64 = 100;

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let Some(tempo) = wait_service("hk-server-tempo") else {
        write(b"hk-server-realtime: no tempo\n");
        exit(1);
    };
    let request = CadenceSnapshot {
        id: 0,
        parent: 0,
        epoch: 0,
        class: CadenceClass::Realtime,
        weight: 1_024,
        budget: millis(1).ticks(),
        period: millis(16).ticks(),
        deadline: millis(16).ticks(),
        latency: 0,
        affinity: CpuMask::ALL.bits(),
        migratable: true,
        lending: Lending::caller_pays(),
    };
    let mut bytes = [0_u8; CadenceSnapshot::ENCODED_LEN];
    request.encode(&mut bytes);
    let Some(handle) = TempoClient::new(tempo, endpoint).admit(&bytes) else {
        write(b"hk-server-realtime: admit failed\n");
        exit(1);
    };
    if task_attach_cadence(0, handle) != 0 {
        write(b"hk-server-realtime: attach failed\n");
        exit(1);
    }

    // 让出 CPU 的同时持续制造可运行负载，观察 EDF 是否在截止期前完成 dispatch。
    let mut attached = [0_u8; CadenceSnapshot::ENCODED_LEN];
    if cadence_self(&mut attached) == CadenceSnapshot::ENCODED_LEN as u64
        && let Ok(snapshot) = CadenceSnapshot::decode(&attached)
    {
        write(b"hk-server-realtime: class=");
        write_u64(u64::from(snapshot.class.as_u8()));
        write(b" budget=");
        write_u64(snapshot.budget);
        write(b" period=");
        write_u64(snapshot.period);
        write(b" deadline=");
        write_u64(snapshot.deadline);
        write(b"\n");
    }

    let start = ticks();
    while ticks().saturating_sub(start) < WINDOW_TICKS {
        for _ in 0..10_000 {
            let _ = tsc();
        }
    }

    let mut bytes = [0_u8; MeterSnapshot::ENCODED_LEN];
    if meter_info(0, &mut bytes) != MeterSnapshot::ENCODED_LEN as u64 {
        write(b"hk-server-realtime: meter failed\n");
        exit(1);
    }
    let Ok(meter) = MeterSnapshot::decode(&bytes) else {
        write(b"hk-server-realtime: meter decode failed\n");
        exit(1);
    };
    write(b"hk-server-realtime: overruns=");
    write_u64(meter.overruns);
    write(b" throttled=");
    write_u64(meter.throttled);
    write(b" consumed=");
    write_u64(meter.consumed);
    write(b" waited=");
    write_u64(meter.waited);
    write(b"\n");
    if meter.overruns != 0 {
        write(b"hk-server-realtime: deadline miss\n");
        exit(1);
    }
    if meter.throttled == 0 {
        write(b"hk-server-realtime: budget not enforced\n");
        exit(1);
    }
    write(b"hk-server-realtime: ok\n");
    exit(0)
}

hk_server_api::server_boilerplate!();
