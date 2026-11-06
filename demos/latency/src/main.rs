//! IPC 往返时延测量：连续调用 echo 服务并输出平均纳秒数。
//!
//! 本 crate 只在 `x86_64-unknown-none` 目标上构建；宿主构建保留占位入口，便于
//! `cargo clippy --workspace --all-targets` 与工作区测试。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use hk_server_api::{call, echo::ENDPOINT_ECHO, exit, tsc, tsc_frequency, write, write_u64};

/// 测量轮数。
#[cfg(target_os = "none")]
const ITERATIONS: u64 = 16;

/// 请求标签。
#[cfg(target_os = "none")]
const REQUEST: u64 = 0x400;

/// 回复标签。
#[cfg(target_os = "none")]
const REPLY: u64 = 0x401;

/// 服务入口：测量 call/reply 往返时延。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(_endpoint: u64) -> ! {
    let start = tsc();
    let mut failures = 0_u64;
    let mut completed = 0_u64;
    while completed < ITERATIONS {
        let (label, word, _sender) = call(ENDPOINT_ECHO, REQUEST, 1);
        if label != REPLY || word != 2 {
            failures += 1;
        }
        completed += 1;
    }
    let cycles = tsc().saturating_sub(start);
    let frequency = tsc_frequency();

    write("hk-server-latency: avg ".as_bytes());
    if frequency == 0 {
        write_u64(cycles / ITERATIONS);
        write(b" cycles\n");
    } else {
        let average = (u128::from(cycles) * 1_000_000_000
            / (u128::from(frequency) * u128::from(ITERATIONS))) as u64;
        write_u64(average);
        write(b" ns\n");
    }
    exit(if failures == 0 { 0 } else { 1 })
}

hk_server_api::server_boilerplate!();
