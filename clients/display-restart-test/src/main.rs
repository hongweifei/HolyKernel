//! display-restart-test：请求 Display 退出并等待其重启，再关闭新实例。
//!
//! 与 Console 一起启动；Console 在自检模式下检测 Display 端点变化并重建窗口。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use hk_proto::display::{DISPLAY_RESTART, DISPLAY_SERVICE, DISPLAY_SHUTDOWN};
#[cfg(target_os = "none")]
use hk_server_api::{exit, send, service_endpoint, ticks, write, write_u64, yield_now};

/// 等待 Console 完成首次连接的 tick 数。
#[cfg(target_os = "none")]
const SETTLE_TICKS: u64 = 50;
/// 等待 Display 重启的 tick 上限。
#[cfg(target_os = "none")]
const RESTART_TICKS: u64 = 1_000;

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(_endpoint: u64) -> ! {
    let display = hk_server_api::runtime::wait_service(DISPLAY_SERVICE).unwrap_or(0);
    if display == 0 {
        write(b"hk-server-display-restart-test: no display\n");
        exit(1)
    }
    write(b"hk-server-display-restart-test: ready ");
    write_u64(display);
    write(b"\n");

    let settle = ticks().saturating_add(SETTLE_TICKS);
    while ticks() < settle {
        let _ = yield_now();
    }
    let _ = send(display, DISPLAY_RESTART, 0);

    let deadline = ticks().saturating_add(RESTART_TICKS);
    let mut gone = false;
    loop {
        let current = service_endpoint(DISPLAY_SERVICE);
        if current == 0 {
            gone = true;
        } else if gone || current != display {
            write(b"hk-server-display-restart-test: restarted ");
            write_u64(current);
            write(b"\n");
            let settle = ticks().saturating_add(SETTLE_TICKS);
            while ticks() < settle {
                let _ = yield_now();
            }
            let _ = send(current, DISPLAY_SHUTDOWN, 0);
            exit(0)
        }
        if ticks() >= deadline {
            write(b"hk-server-display-restart-test: timeout\n");
            exit(2)
        }
        let _ = yield_now();
    }
}

hk_server_api::server_boilerplate!();
