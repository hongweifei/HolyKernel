//! OOM 冒烟观察端：轮询最近一次审计事件，确认 victim 被终止后退出。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use hk_user_api::{exit, memory_event_count, oom_event, sleep_ticks, write};

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(_endpoint: u64, _capacity: u64) -> ! {
    let mut buffer = [0_u8; 96];
    loop {
        if let Some(length) = oom_event(&mut buffer)
            && length > 40
        {
            let killed = u64::from_le_bytes(buffer[16..24].try_into().unwrap_or([0; 8]));
            if killed != 0 {
                let name_end = length.min(buffer.len());
                write(b"hk-client-oom-observer: victim ");
                write(&buffer[40..name_end]);
                write(b"\n");
                hk_user_api::format::Line::new()
                    .text(b"hk-client-oom-observer: events ")
                    .number(memory_event_count() as u64)
                    .text(b"\n")
                    .finish();
                write(b"hk-client-oom-observer: ok\n");
                exit(0)
            }
        }
        sleep_ticks(5);
    }
}

hk_user_api::server_boilerplate!();
