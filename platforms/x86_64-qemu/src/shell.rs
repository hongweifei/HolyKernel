//! HolyShell：脚本化与交互式命令执行。
//!
//! 命令解析与输出文本由 `hk-shell` 提供；本模块负责读取输入、写出到串口与
//! 帧缓冲控制台，并执行清屏与退出等平台副作用。

use alloc::format;
use alloc::string::String;

use hk_hal::Console;
use hk_platform_x86_64::{FramebufferConsole, Serial, keyboard};
use hk_root_server::ServiceTable;
use hk_shell::{execute, parse};

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
        if execute_line(console, screen, services, line) {
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
        if execute_line(console, screen, services, command) {
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

fn execute_line(
    console: &Serial,
    screen: Option<&FramebufferConsole>,
    services: &ServiceTable,
    line: &str,
) -> bool {
    let response = execute(parse(line), services);
    if response.clear {
        if let Some(screen) = screen {
            screen.clear();
        }
    }
    write(console, screen, &response.output);
    response.exit
}

fn write(console: &Serial, screen: Option<&FramebufferConsole>, text: &str) {
    console.write_str(text);
    if let Some(screen) = screen {
        screen.write_str(text);
    }
}
