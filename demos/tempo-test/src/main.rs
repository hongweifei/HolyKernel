//! Tempo 自检：向 Tempo 申请一个收窄子契约，附加到自身并读回验证。
//!
//! 成功输出 `hk-server-tempo-test: ok weight=512`。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use hk_cadence::{CadenceClass, CadenceSnapshot, CpuMask, Lending};
#[cfg(target_os = "none")]
use hk_proto_client::TempoClient;
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::{
    cadence_self, exit, runtime::wait_service, task_attach_cadence, write, write_u64,
};

/// 服务堆大小。
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 64 * 1024;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

/// 申请的子契约权重。
#[cfg(target_os = "none")]
const REQUEST_WEIGHT: u32 = 512;

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let Some(tempo) = wait_service("hk-server-tempo") else {
        write(b"hk-server-tempo-test: no tempo\n");
        exit(1);
    };
    let request = CadenceSnapshot {
        id: 0,
        parent: 0,
        epoch: 0,
        class: CadenceClass::Latency,
        weight: REQUEST_WEIGHT,
        budget: 1_000,
        period: 16_000,
        deadline: 0,
        latency: 0,
        affinity: CpuMask::ALL.bits(),
        migratable: true,
        lending: Lending::caller_pays(),
    };
    let mut bytes = [0_u8; CadenceSnapshot::ENCODED_LEN];
    request.encode(&mut bytes);

    let Some(handle) = TempoClient::new(tempo, endpoint).admit(&bytes) else {
        write(b"hk-server-tempo-test: admit failed\n");
        exit(1);
    };
    if task_attach_cadence(0, handle) != 0 {
        write(b"hk-server-tempo-test: attach failed\n");
        exit(1);
    }

    let mut attached = [0_u8; CadenceSnapshot::ENCODED_LEN];
    if cadence_self(&mut attached) != CadenceSnapshot::ENCODED_LEN as u64 {
        write(b"hk-server-tempo-test: info failed\n");
        exit(1);
    }
    let Ok(attached) = CadenceSnapshot::decode(&attached) else {
        write(b"hk-server-tempo-test: decode failed\n");
        exit(1);
    };
    if attached.weight != REQUEST_WEIGHT {
        write(b"hk-server-tempo-test: wrong weight\n");
        exit(1);
    }

    let mut client = TempoClient::new(tempo, endpoint);
    for (weight, regrant) in [(1_024_u32, false), (REQUEST_WEIGHT, true)] {
        let mut request = request;
        request.weight = weight;
        request.budget = 2_000;
        let mut encoded = [0_u8; CadenceSnapshot::ENCODED_LEN];
        request.encode(&mut encoded);
        if !client.admit_named("hk-server-tempo-test", &encoded, regrant) {
            write(b"hk-server-tempo-test: named failed\n");
            exit(1);
        }
        let mut snapshot = [0_u8; CadenceSnapshot::ENCODED_LEN];
        if cadence_self(&mut snapshot) != CadenceSnapshot::ENCODED_LEN as u64 {
            write(b"hk-server-tempo-test: named info failed\n");
            exit(1);
        }
        let Ok(snapshot) = CadenceSnapshot::decode(&snapshot) else {
            write(b"hk-server-tempo-test: named decode failed\n");
            exit(1);
        };
        if snapshot.weight != weight {
            write(b"hk-server-tempo-test: named wrong weight\n");
            exit(1);
        }
    }

    write(b"hk-server-tempo-test: ok weight=");
    write_u64(u64::from(attached.weight));
    write(b"\n");
    exit(0)
}

hk_server_api::server_boilerplate!();
