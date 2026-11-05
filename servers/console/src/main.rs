//! 控制台服务：读取内核输入、做行编辑与回显，并向客户端提供写文本/读一行。
//!
//! 有 Display 服务时，Console 创建自己的窗口，把文本渲染到共享表面并提交
//! 合成；没有 Display 时回退到内核控制台输出。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use hk_server_api::console::{
    CONSOLE_READ_LINE, CONSOLE_READ_REQUEST, CONSOLE_SHUTDOWN, CONSOLE_WRITE, CONSOLE_WRITE_ACK,
};
#[cfg(target_os = "none")]
use hk_server_api::display::{DISPLAY_FRAME_DONE, DISPLAY_KEY, DISPLAY_SERVICE, unpack_key};
#[cfg(target_os = "none")]
use hk_server_api::{
    console_read, exit, recv_data_try, send, send_data, service_endpoint, write, write_u64,
    yield_now,
};
#[cfg(target_os = "none")]

/// 输出到图形终端；没有终端时退回内核控制台。
#[cfg(target_os = "none")]
fn emit(terminal: &mut Option<Terminal>, text: &[u8]) {
    match terminal {
        Some(terminal) => terminal.emit(text),
        None => write(text),
    }
}

/// 处理一个输入字节：行编辑、回显与按行回复。
#[cfg(target_os = "none")]
fn process_command(terminal: &mut Option<Terminal>, command: &str) {
    let command = command.trim();
    if command.is_empty() {
        return;
    }
    if command == "help" {
        emit(
            terminal,
            "命令：help, echo <text>, clear, version, services\n".as_bytes(),
        );
    } else if command == "clear" {
        if let Some(terminal) = terminal {
            terminal.clear();
        }
    } else if command == "version" {
        emit(terminal, "HolyKernel M6 · Console\n".as_bytes());
    } else if let Some(text) = command.strip_prefix("echo ") {
        emit(terminal, text.as_bytes());
        emit(terminal, b"\n");
    } else if command == "services" {
        for name in [
            "hk-server-display",
            "hk-server-console",
            "hk-server-input",
            "hk-server-device",
            "hk-server-holydoc",
            "hk-server-net",
            "hk-server-vfs",
        ] {
            emit(terminal, name.as_bytes());
            emit(
                terminal,
                if service_endpoint(name) == 0 {
                    b": offline\n".as_slice()
                } else {
                    b": online\n".as_slice()
                },
            );
        }
    } else if command == "exit" {
        emit(
            terminal,
            "桌面 Console 常驻；请使用标题栏关闭窗口\n".as_bytes(),
        );
    } else if command == "tasks" || command == "mounts" || command == "handles" {
        emit(
            terminal,
            "桌面 Console 暂不支持该命令；请使用文本模式\n".as_bytes(),
        );
    } else {
        emit(terminal, "未知命令：".as_bytes());
        emit(terminal, command.as_bytes());
        emit(terminal, "（help 查看）\n".as_bytes());
    }
}

#[cfg(target_os = "none")]
fn handle_input(
    terminal: &mut Option<Terminal>,
    line: &mut [u8; 128],
    length: &mut usize,
    pending: &mut u64,
    byte: u8,
) {
    match byte {
        b'\n' | b'\r' => {
            emit(terminal, b"\n");
            if *pending != 0 {
                let _ = send_data(*pending, CONSOLE_READ_LINE, &line[..*length]);
                *pending = 0;
                *length = 0;
                return;
            }
            let command = core::str::from_utf8(&line[..*length]).unwrap_or("");
            process_command(terminal, command);
            emit(terminal, b"> ");
            *length = 0;
        }
        0x08 | 0x7F => {
            if *length > 0 {
                *length -= 1;
                emit(terminal, b"\x08 \x08");
            }
        }
        byte if byte >= 0x20 && byte != 0x7F => {
            if *length < line.len() {
                line[*length] = byte;
                *length += 1;
                emit(terminal, &[byte]);
            }
        }
        _ => {}
    }
}

#[cfg(target_os = "none")]
mod terminal;

#[cfg(target_os = "none")]
use terminal::{Terminal, attach_terminal};

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let mut terminal = attach_terminal(endpoint);
    if let Some(terminal) = &mut terminal {
        write(b"hk-server-console: window ");
        write_u64(terminal.window());
        write(b"\n");
        hk_server_api::display::set_title(terminal.display(), terminal.window(), "Console");
        terminal.emit(b"> ");
    }

    let mut ever_attached = terminal.is_some();
    let mut reconnected = false;
    let mut cmdline_buffer = [0_u8; 128];
    let restart_test = hk_server_api::boot_cmdline(&mut cmdline_buffer)
        .is_some_and(|line| line.contains("display-restart-test"));
    let mut buffer = [0_u8; 256];
    let mut line = [0_u8; 128];
    let mut length = 0_usize;
    let mut pending = 0_u64;

    loop {
        let current_display = service_endpoint(DISPLAY_SERVICE);
        let needs_attach = match terminal.as_ref() {
            Some(terminal) => terminal.display() != current_display,
            None => current_display != 0,
        };
        if needs_attach {
            if let Some(terminal) = &terminal {
                terminal.detach();
            }
            terminal = attach_terminal(endpoint);
            if let Some(terminal) = &mut terminal {
                if ever_attached && !reconnected {
                    write(b"hk-server-console: reconnected\n");
                    reconnected = true;
                }
                write(b"hk-server-console: window ");
                write_u64(terminal.window());
                write(b"\n");
                hk_server_api::display::set_title(terminal.display(), terminal.window(), "Console");
                terminal.emit(b"> ");
                ever_attached = true;
            }
        }
        if restart_test && reconnected && current_display == 0 {
            write(b"hk-server-console: done\n");
            exit(0)
        }
        loop {
            let (label, word, sender, size) = recv_data_try(endpoint, &mut buffer);
            if label == 0 {
                break;
            }
            match label {
                CONSOLE_WRITE => {
                    let text = &buffer[..size.min(buffer.len())];
                    emit(&mut terminal, text);
                    if terminal.is_some() {
                        let _ = send(sender, CONSOLE_WRITE_ACK, 0);
                    }
                }
                CONSOLE_READ_REQUEST => pending = sender,
                CONSOLE_SHUTDOWN => exit(0),
                DISPLAY_KEY => {
                    let (_, byte) = unpack_key(word);
                    handle_input(&mut terminal, &mut line, &mut length, &mut pending, byte);
                }
                DISPLAY_FRAME_DONE => {}
                _ => {}
            }
        }

        // 有 Display 时，键盘事件经焦点窗口路由到本服务；无 Display 时直接读内核输入。
        if terminal.is_none() {
            while let Some(byte) = console_read() {
                handle_input(&mut terminal, &mut line, &mut length, &mut pending, byte);
            }
        }

        let _ = yield_now();
    }
}

hk_server_api::server_boilerplate!();
