//! hk-dbg：打印任务/服务/trace/统计与最近一次异常回溯。
//!
//! 作为独立用户态服务运行，只通过 Root Server 控制协议读取视图；结果输出到
//! 串口，便于无图形环境观察。模块名为 `hk-server-dbg`。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::root::{
    ROOT_OP_HANDLES, ROOT_OP_SERVICES, ROOT_OP_STATS, ROOT_OP_TASKS, ROOT_OP_TRACE, call,
};
#[cfg(target_os = "none")]
use hk_server_api::{exit, write};

/// 服务堆大小。
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 64 * 1024;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

/// Root 控制结果缓冲长度。
#[cfg(target_os = "none")]
const BUFFER: usize = 32 * 1024;

/// 输出一个视图。
#[cfg(target_os = "none")]
fn print(title: &str, op: u64) {
    write(b"hk-server-dbg: ");
    write(title.as_bytes());
    write(b"\n");
    let mut buffer = [0_u8; BUFFER];
    match call(op, "", &mut buffer) {
        Ok(length) => write(&buffer[..length.min(buffer.len())]),
        Err(code) => {
            write(b"hk-server-dbg: control failed ");
            hk_server_api::write_u64(code);
            write(b"\n");
        }
    }
}

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let _ = hk_server_api::fault_subscribe(endpoint);
    write(b"hk-server-dbg: ready\n");
    let mut events = [0_u8; 2048];
    for _ in 0..64 {
        let (label, _word, _sender, length) = hk_server_api::recv_data_try(endpoint, &mut events);
        if label != 0 {
            hk_debug::ingest(label, &events[..length]);
        }
        let _ = hk_server_api::yield_now();
    }
    print("tasks", ROOT_OP_TASKS);
    print("handles", ROOT_OP_HANDLES);
    print("services", ROOT_OP_SERVICES);
    print("trace", ROOT_OP_TRACE);
    print("stats", ROOT_OP_STATS);
    write(b"hk-server-dbg: fault\n");
    write(hk_debug::fault_text().as_bytes());
    write(b"hk-server-dbg: done\n");
    exit(0)
}

hk_server_api::server_boilerplate!();
