//! Block Server：RAM disk 块设备服务。
//!
//! virtio-blk 由独立的 `hk-server-virtio` 驱动服务暴露；本服务保留内存块设备，
//! 为测试与恢复路径提供 Block Endpoint。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use hk_block::{MemoryDisk, SECTOR_SIZE};
#[cfg(target_os = "none")]
use hk_block_ipc::serve;
#[cfg(target_os = "none")]
use hk_user_api::{exit, write};

hk_user_api::server_heap!();
/// 内存盘块数。
#[cfg(target_os = "none")]
const BLOCKS: u64 = 64;

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    write("hk-server-block: ram disk ready\n".as_bytes());
    let mut disk = MemoryDisk::new(BLOCKS, SECTOR_SIZE);
    serve(endpoint, &mut disk);
    exit(0)
}

hk_user_api::server_boilerplate!();
