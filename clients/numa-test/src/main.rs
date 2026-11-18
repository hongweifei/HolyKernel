//! NUMA 迁移冒烟：在节点 0 装入一页，迁移到节点 1，再校验内容。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use hk_user_api::{MemoryClient, exit, numa_migrate, numa_node_count, write};

#[cfg(target_os = "none")]
const PAGE: usize = 4096;

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64, _capacity: u64) -> ! {
    if numa_node_count().unwrap_or(0) < 2 {
        write(b"hk-client-numa-test: need two nodes\n");
        exit(1)
    }
    let client = MemoryClient::new(endpoint);
    let Some((capability, _region, virt)) = client.map_lazy(PAGE) else {
        write(b"hk-client-numa-test: map failed\n");
        exit(2)
    };
    // SAFETY: Region 已映射；先读触发零页装入，再写入 pattern。
    let _zero = unsafe { core::ptr::read_volatile(virt as *const u8) };
    let pattern = 0xA7_u8;
    // SAFETY: 上一行读取已触发零页装入；此处写同一 Region 内已映射的 1 字节。
    unsafe {
        core::ptr::write_volatile(virt as *mut u8, pattern);
    }
    let migrated = numa_migrate(capability, 0, PAGE, 1);
    if migrated != PAGE as u64 {
        write(b"hk-client-numa-test: migrate failed\n");
        exit(3)
    }
    // SAFETY: 迁移已解除旧 PTE；读取触发按新 Frame 重建映射。
    let restored = unsafe { core::ptr::read_volatile(virt as *const u8) };
    if restored != pattern {
        write(b"hk-client-numa-test: content mismatch\n");
        exit(4)
    }
    hk_user_api::format::Line::new()
        .text(b"hk-client-numa-test: migrated ")
        .number(migrated)
        .text(b" verified\n")
        .finish();
    write(b"hk-client-numa-test: ok\n");
    exit(0)
}

hk_user_api::server_boilerplate!();
