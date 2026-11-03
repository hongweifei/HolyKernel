//! HolyShell 基础命令。
//!
//! 当前提供脚本化执行：内核命令行以 `shell-test` 开头时，后续词按顺序作为命令
//! 执行。命令输出同时写到串口与帧缓冲控制台。

use alloc::format;
use alloc::string::String;

use hk_hal::Console;
use hk_platform_x86_64::{FramebufferConsole, Serial, keyboard};

use crate::service::ServiceTable;

/// 执行一组命令。
pub fn run(
    console: &Serial,
    screen: Option<&FramebufferConsole>,
    services: &ServiceTable,
    script: &[&str],
) -> Result<(), String> {
    write(console, screen, "HolyShell\n");

    if script.is_empty() {
        write(
            console,
            screen,
            "当前仅支持脚本化执行：shell-test <命令>...\n",
        );
        return Ok(());
    }

    for line in script {
        if line.is_empty() {
            continue;
        }
        write(console, screen, &format!("hk> {line}\n"));
        if execute(console, screen, services, line)? {
            break;
        }
    }
    Ok(())
}

/// 运行交互式 HolyShell：从键盘或串口读取命令行。
pub fn interactive(
    console: &Serial,
    screen: Option<&FramebufferConsole>,
    services: &ServiceTable,
) -> Result<(), String> {
    write(console, screen, "HolyShell 交互模式（help 查看命令）\n");

    loop {
        write(console, screen, "hk> ");
        let line = read_line(console, screen);
        let command = line.trim();
        if command.is_empty() {
            continue;
        }
        if execute(console, screen, services, command)? {
            break;
        }
    }
    Ok(())
}

fn read_line(console: &Serial, screen: Option<&FramebufferConsole>) -> String {
    let mut line = String::new();
    loop {
        let byte = wait_byte(console);
        match byte {
            b'\n' | b'\r' => {
                write(console, screen, "\n");
                return line;
            }
            0x08 | 0x7F => {
                if line.pop().is_some() {
                    console.write_str("\u{8} \u{8}");
                    if let Some(screen) = screen {
                        screen.backspace();
                    }
                }
            }
            byte if (0x20..0x7F).contains(&byte) => {
                line.push(char::from(byte));
                write(
                    console,
                    screen,
                    core::str::from_utf8(&[byte]).unwrap_or_default(),
                );
            }
            _ => {}
        }
    }
}

fn wait_byte(console: &Serial) -> u8 {
    loop {
        if let Some(byte) = keyboard::read() {
            return byte;
        }
        if let Some(byte) = console.try_read_byte() {
            return byte;
        }
        // SAFETY: STI 之后的指令才响应中断，`hlt` 在等待期间让出 CPU；
        // 键盘中断或串口中断会唤醒等待。
        unsafe {
            core::arch::asm!("sti; hlt; cli", options(nomem, nostack));
        }
    }
}

fn execute(
    console: &Serial,
    screen: Option<&FramebufferConsole>,
    services: &ServiceTable,
    line: &str,
) -> Result<bool, String> {
    let mut parts = line.split_whitespace();
    let command = parts.next().unwrap_or_default();
    let arguments: String = parts.collect::<alloc::vec::Vec<_>>().join(" ");

    match command {
        "help" => {
            write(
                console,
                screen,
                "命令：help、services、mounts、clear、echo <文本>、exit\n",
            );
        }
        "services" => list_services(console, screen, services),
        "mounts" => {
            write(console, screen, "尚无挂载（VFS 在 M4 接入）\n");
        }
        "clear" => {
            if let Some(screen) = screen {
                screen.clear();
            }
        }
        "echo" => {
            write(console, screen, &format!("{arguments}\n"));
        }
        "exit" => return Ok(true),
        other => {
            write(console, screen, &format!("未知命令：{other}\n"));
        }
    }
    Ok(false)
}

fn list_services(console: &Serial, screen: Option<&FramebufferConsole>, services: &ServiceTable) {
    if services.is_empty() {
        write(console, screen, "服务表为空\n");
        return;
    }

    write(
        console,
        screen,
        &format!("服务表（{} 项）\n", services.len()),
    );
    write(console, screen, "名称              状态      退出码\n");
    for service in services.services() {
        let exit = match service.exit_code {
            Some(code) => format!("{code}"),
            None => String::from("-"),
        };
        write(
            console,
            screen,
            &format!(
                "{:<16}  {:<8}  {}\n",
                service.name,
                service.state.name(),
                exit
            ),
        );
    }
}

fn write(console: &Serial, screen: Option<&FramebufferConsole>, text: &str) {
    console.write_str(text);
    if let Some(screen) = screen {
        screen.write_str(text);
    }
}
