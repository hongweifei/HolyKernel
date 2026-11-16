//! Gpu DeviceAllocation：私有 Gpu Capacity、懒分配与 Provider 驻留。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use hk_server_api::{
    CAPACITY_KIND_GPU, CapabilityKind, PROVIDER_KIND_DEVICE, capability_bind_provider,
    capability_create, capability_create_lazy, capability_phys, exit, frame_bank_stats, map_lazy,
    runtime::wait_service, write,
};

#[cfg(target_os = "none")]
const PAGE: usize = 4096;
/// Provider 写入的测试 pattern。
#[cfg(target_os = "none")]
const PATTERN: u8 = 0x5A;

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(_endpoint: u64, _capacity: u64) -> ! {
    let Some(provider) = wait_service("hk-server-gpu-provider") else {
        write(b"hk-client-gpu-test: provider missing\n");
        exit(2)
    };
    let Some(eager) = capability_create(CapabilityKind::DeviceAllocation, PAGE) else {
        write(b"hk-client-gpu-test: Gpu allocation failed\n");
        exit(3)
    };
    if capability_phys(eager).is_none() {
        write(b"hk-client-gpu-test: Gpu backing missing\n");
        exit(4)
    }
    let Some(lazy) = capability_create_lazy(CapabilityKind::DeviceAllocation, PAGE) else {
        write(b"hk-client-gpu-test: lazy allocation failed\n");
        exit(5)
    };
    if capability_bind_provider(lazy, provider, PROVIDER_KIND_DEVICE) != 0 {
        write(b"hk-client-gpu-test: bind failed\n");
        exit(6)
    }
    let Some((_region, virt)) = map_lazy(lazy, PAGE) else {
        write(b"hk-client-gpu-test: map failed\n");
        exit(7)
    };
    // SAFETY: Region 已映射；首次读触发 Provider 驻留并安装 Frame。
    let value = unsafe { core::ptr::read_volatile(virt as *const u8) };
    if value != PATTERN {
        write(b"hk-client-gpu-test: provider page mismatch\n");
        exit(8)
    }
    let gpu = frame_bank_stats()
        .map(|stats| stats.by_capacity[CAPACITY_KIND_GPU as usize])
        .unwrap_or(0);
    if gpu == 0 {
        write(b"hk-client-gpu-test: Gpu usage missing\n");
        exit(9)
    }
    write(b"hk-client-gpu-test: provider page ok\n");
    hk_server_api::format::Line::new()
        .text(b"hk-client-gpu-test: gpu bytes ")
        .number(gpu)
        .text(b"\n")
        .finish();
    write(b"hk-client-gpu-test: ok\n");
    exit(0)
}

hk_server_api::server_boilerplate!();
