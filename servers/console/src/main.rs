//! 控制台服务：读取内核输入、做行编辑与回显，并向客户端提供写文本/读一行。
//!
//! 有 Display 服务时，Console 创建自己的窗口，把文本渲染到共享表面并提交
//! 合成；没有 Display 时回退到内核控制台输出。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use hk_proto::console::{
    CONSOLE_CLEAR, CONSOLE_PING, CONSOLE_PONG, CONSOLE_READ_REQUEST, CONSOLE_SHUTDOWN,
    CONSOLE_WRITE, CONSOLE_WRITE_ACK,
};
#[cfg(target_os = "none")]
use hk_proto::display::{
    DISPLAY_KEY, DISPLAY_RESIZE, DISPLAY_SERVICE, SURFACE_FRAME_DONE, unpack_key,
    unpack_window_size,
};
#[cfg(target_os = "none")]
use hk_proto::input::{Decoder, Key};
#[cfg(target_os = "none")]
use hk_server_api::{
    console_read, exit, recv_data_try, send, send_data, service_endpoint, write, write_u64,
    yield_now,
};

#[cfg(target_os = "none")]
mod editor;
#[cfg(target_os = "none")]
mod terminal;

#[cfg(target_os = "none")]
use editor::{Editor, emit};
#[cfg(target_os = "none")]
use terminal::attach_terminal;

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let mut terminal = attach_terminal(endpoint);
    if let Some(terminal) = &mut terminal {
        write(b"hk-server-console: window ");
        write_u64(terminal.window());
        write(b"\n");
        hk_proto::display::set_title(terminal.display(), terminal.window(), "Console");
    }

    let mut ever_attached = terminal.is_some();
    let mut reconnected = false;
    let mut cmdline_buffer = [0_u8; 128];
    let restart_test = hk_server_api::boot_cmdline(&mut cmdline_buffer)
        .is_some_and(|line| line.contains("display-restart-test"));
    let mut buffer = [0_u8; 256];
    let mut editor = Editor::new();
    let mut keys = Decoder::new();

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
                hk_proto::display::set_title(terminal.display(), terminal.window(), "Console");
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
                CONSOLE_READ_REQUEST => editor.request(sender),
                CONSOLE_PING => {
                    let _ = send_data(sender, CONSOLE_PONG, &[]);
                }
                CONSOLE_CLEAR => {
                    if let Some(terminal) = &mut terminal {
                        terminal.clear();
                    }
                }
                CONSOLE_SHUTDOWN => exit(0),
                DISPLAY_KEY => {
                    let (_, byte) = unpack_key(word);
                    if let Some(Key::Byte(byte)) = keys.feed(byte) {
                        editor.handle(&mut terminal, byte);
                    }
                }
                DISPLAY_RESIZE => {
                    if let Some(terminal) = &mut terminal {
                        let (_, width, height) = unpack_window_size(word);
                        terminal.resize(width as usize, height as usize);
                    }
                }
                SURFACE_FRAME_DONE => {}
                _ => {}
            }
        }

        // 有 Display 时，键盘事件经焦点窗口路由到本服务；无 Display 时直接读内核输入。
        if terminal.is_none() {
            while let Some(byte) = console_read() {
                editor.handle(&mut terminal, byte);
            }
        }

        let _ = yield_now();
    }
}

hk_server_api::server_boilerplate!();
