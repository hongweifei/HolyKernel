//! Block Server：RAM disk 块设备服务。
//!
//! virtio-blk 由独立的 `hk-server-virtio` 驱动服务暴露；本服务保留内存块设备，
//! 为测试与恢复路径提供 Block Endpoint。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use core::panic::PanicInfo;

#[cfg(target_os = "none")]
use hk_block::ipc::serve;
#[cfg(target_os = "none")]
use hk_block::{MemoryDisk, SECTOR_SIZE};
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::{exit, write};

/// 服务堆大小。
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 32 * 1024;
/// 内存盘块数。
#[cfg(target_os = "none")]
const BLOCKS: u64 = 64;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    write("hk-server-block: ram disk ready\n".as_bytes());
    let mut disk = MemoryDisk::new(BLOCKS, SECTOR_SIZE);
    serve(endpoint, &mut disk);
    exit(0)
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(not(target_os = "none"))]
fn main() {
    eprintln!("hk-server-block 只在 x86_64-unknown-none 目标上构建");
}
