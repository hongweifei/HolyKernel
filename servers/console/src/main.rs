//! 控制台服务：读取内核输入、做行编辑与回显，并向客户端提供写文本/读一行。
//!
//! 硬件输入由内核转发（`SYS_CONSOLE_READ` + 输入通知），服务本身不拥有 IRQ。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;
#[cfg(target_os = "none")]
use hk_server_api::console::{
    CONSOLE_NOTIFY_INPUT, CONSOLE_NOTIFY_MESSAGE, CONSOLE_READ_LINE, CONSOLE_READ_REQUEST,
    CONSOLE_SHUTDOWN, CONSOLE_WRITE,
};
#[cfg(target_os = "none")]
use hk_server_api::{
    console_read, console_subscribe, exit, recv_data_try, self_notification, send_data,
    wait_notify, write,
};

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let _ = console_subscribe();
    let notification = self_notification();
    let mut buffer = [0_u8; 256];
    let mut line = [0_u8; 128];
    let mut length = 0_usize;
    let mut pending = 0_u64;

    loop {
        loop {
            let (label, _word, sender, size) = recv_data_try(endpoint, &mut buffer);
            if label == 0 {
                break;
            }
            match label {
                CONSOLE_WRITE => write(&buffer[..size.min(buffer.len())]),
                CONSOLE_READ_REQUEST => pending = sender,
                CONSOLE_SHUTDOWN => exit(0),
                _ => {}
            }
        }

        while let Some(byte) = console_read() {
            match byte {
                b'\n' | b'\r' => {
                    write(b"\n");
                    if pending != 0 {
                        let _ = send_data(pending, CONSOLE_READ_LINE, &line[..length]);
                        pending = 0;
                    }
                    length = 0;
                }
                0x08 | 0x7F => {
                    if length > 0 {
                        length -= 1;
                        write(b"\x08 \x08");
                    }
                }
                byte if (0x20..0x7F).contains(&byte) => {
                    if length < line.len() {
                        line[length] = byte;
                        length += 1;
                        write(&[byte]);
                    }
                }
                _ => {}
            }
        }

        let _ = wait_notify(notification, CONSOLE_NOTIFY_INPUT | CONSOLE_NOTIFY_MESSAGE);
    }
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
    eprintln!("hk-server-console 只在 x86_64-unknown-none 目标上构建");
}
