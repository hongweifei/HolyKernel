//! OOM 冒烟触发端：设置高阈值，用一次超过 Capacity 配额的分配触发 victim 选择。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use hk_server_api::{capacity_alloc, exit, set_oom_policy, write};

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(_endpoint: u64, capacity: u64) -> ! {
    if set_oom_policy(99, 1) != 0 {
        write(b"hk-server-oom-test: policy failed\n");
        exit(1)
    }
    write(b"hk-server-oom-test: triggering\n");
    // 超过顶层 User Capacity 配额，触发 OOM guard；victim 选择应终止本任务。
    let _ = capacity_alloc(capacity, 1024 * 1024 * 1024);
    write(b"hk-server-oom-test: not killed\n");
    exit(2)
}

hk_server_api::server_boilerplate!();
