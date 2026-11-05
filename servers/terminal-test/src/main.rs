//! terminal-test：向 Console 写入文本，确认图形终端完成一次合成。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#[cfg(target_os = "none")]
use core::panic::PanicInfo;
#[cfg(target_os = "none")]
use hk_server_api::console::{
    CONSOLE_READ_LINE, CONSOLE_READ_REQUEST, CONSOLE_SHUTDOWN, CONSOLE_WRITE, CONSOLE_WRITE_ACK,
};
#[cfg(target_os = "none")]
use hk_server_api::display::{DISPLAY_SERVICE, DISPLAY_SHUTDOWN};
#[cfg(target_os = "none")]
use hk_server_api::{
    exit, recv_data_from, recv_data_try, send, send_data, service_endpoint, write, yield_now,
};

/// 等待服务出现的重试次数。
#[cfg(target_os = "none")]
const WAIT_ATTEMPTS: usize = 4096;

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let console = wait_for("hk-server-console");
    if console == 0 {
        write(b"hk-server-terminal-test: console not found\n");
        exit(1)
    }
    let display = wait_for(DISPLAY_SERVICE);
    if display == 0 {
        write(b"hk-server-terminal-test: display not found\n");
        exit(1)
    }

    // Console 启动早期可能正在等待 Display 的窗口回复；重发直到收到确认。
    let mut payload = [0_u8; 8];
    let mut acknowledged = false;
    for _ in 0..2048 {
        let _ = send_data(console, CONSOLE_WRITE, b"hello terminal\n");
        for _ in 0..64 {
            let (label, _word, _sender, _size) = recv_data_try(endpoint, &mut payload);
            if label == CONSOLE_WRITE_ACK {
                acknowledged = true;
                break;
            }
            let _ = yield_now();
        }
        if acknowledged {
            break;
        }
    }
    if !acknowledged {
        write(b"hk-server-terminal-test: no ack\n");
        exit(2)
    }

    // 请求一行输入；键盘事件应经 Display 焦点路由到 Console。
    let _ = send(console, CONSOLE_READ_REQUEST, 0);
    let mut line = [0_u8; 64];
    let (label, _word, _sender, length) = recv_data_from(endpoint, console, &mut line);
    if label != CONSOLE_READ_LINE {
        write(b"hk-server-terminal-test: no line\n");
        exit(3)
    }
    write(b"hk-server-terminal-test: line ");
    write(&line[..length.min(line.len())]);
    write(b"\n");

    write(b"hk-server-terminal-test: done\n");
    let _ = send(console, CONSOLE_SHUTDOWN, 0);
    let _ = send(display, DISPLAY_SHUTDOWN, 0);
    exit(0)
}

#[cfg(target_os = "none")]
fn wait_for(name: &str) -> u64 {
    for _ in 0..WAIT_ATTEMPTS {
        let endpoint = service_endpoint(name);
        if endpoint != 0 {
            return endpoint;
        }
        let _ = yield_now();
    }
    0
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
    eprintln!("hk-server-terminal-test 只在 x86_64-unknown-none 目标上构建");
}
