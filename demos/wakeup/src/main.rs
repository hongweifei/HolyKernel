//! 唤醒延迟探针。
//!
//! 连续调用 `SYS_SLEEP(1)`（10 ms），用 TSC 测量“到期到任务重新运行”的额外
//! 延迟，输出 min/avg/max。平均超过 2 ms 视为回归。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::{exit, sleep_ticks, tsc, tsc_frequency, write, write_u64};

/// 服务堆大小。
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 32 * 1024;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

/// 测量轮数。
#[cfg(target_os = "none")]
const ROUNDS: u64 = 64;

/// 每次休眠的契约时长（微秒）：1 个 10 ms 调度 tick。
#[cfg(target_os = "none")]
const REQUEST_US: u64 = 10_000;

/// 平均延迟上限（微秒）。
#[cfg(target_os = "none")]
const MAX_AVERAGE_US: u64 = 2_000;

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(_endpoint: u64) -> ! {
    let frequency = tsc_frequency();
    if frequency == 0 {
        write(b"hk-server-wakeup: tsc unavailable\n");
        exit(1);
    }

    let mut min = u64::MAX;
    let mut max = 0_u64;
    let mut sum = 0_u64;
    for _ in 0..ROUNDS {
        let start = tsc();
        let _ = sleep_ticks(1);
        let end = tsc();
        let elapsed_us =
            (u128::from(end.saturating_sub(start)) * 1_000_000 / u128::from(frequency)) as u64;
        let latency = elapsed_us.saturating_sub(REQUEST_US);
        min = min.min(latency);
        max = max.max(latency);
        sum = sum.saturating_add(latency);
    }
    let average = sum / ROUNDS;

    write(b"hk-server-wakeup: min=");
    write_u64(min);
    write(b"us avg=");
    write_u64(average);
    write(b"us max=");
    write_u64(max);
    write(b"us\n");
    if average > MAX_AVERAGE_US {
        write(b"hk-server-wakeup: too slow\n");
        exit(1);
    }
    write(b"hk-server-wakeup: ok\n");
    exit(0)
}

hk_server_api::server_boilerplate!();
