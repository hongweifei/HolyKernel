//! Gpu Provider：从 Gpu Capacity 分配 Frame，为 DeviceAllocation 提供驻留页。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use hk_server_api::{
    PROVIDER_ERROR, PROVIDER_EVICT, PROVIDER_FAULT, PROVIDER_PROVIDE, PROVIDER_RELEASE,
    PROVIDER_WRITEBACK, capacity_alloc, frame_write, recv_data_handle, reply, reply_handle, write,
};

/// 页长度。
#[cfg(target_os = "none")]
const PAGE: usize = 4096;
/// Provider 驻留页的测试 pattern。
#[cfg(target_os = "none")]
const PATTERN: u8 = 0x5A;

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(_endpoint: u64, capacity: u64) -> ! {
    write(b"hk-server-gpu-provider: ready\n");
    let mut buffer = [0_u8; 32];
    loop {
        let (label, _word, _sender, _length, _handle) = recv_data_handle(_endpoint, &mut buffer, 0);
        if label == PROVIDER_FAULT {
            let Some(frame) = capacity_alloc(capacity, PAGE) else {
                let _ = reply(0, u64::MAX);
                continue;
            };
            let page = [PATTERN; PAGE];
            if frame_write(frame, 0, &page) != 0 {
                let _ = reply(0, u64::MAX);
                continue;
            }
            if reply_handle(PROVIDER_PROVIDE, 0, frame) != 0 {
                hk_server_api::exit(2)
            }
            continue;
        }
        if matches!(
            label,
            PROVIDER_RELEASE | PROVIDER_EVICT | PROVIDER_WRITEBACK | PROVIDER_ERROR
        ) {
            let _ = reply(0, 0);
            continue;
        }
        let _ = reply(0, u64::MAX);
    }
}

hk_server_api::server_boilerplate!();
