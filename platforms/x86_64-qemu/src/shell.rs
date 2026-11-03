//! HolyShell：脚本化与交互式命令执行。
//!
//! 命令解析与输出文本由 `hk-shell` 提供；本模块负责读取输入、写出到串口与
//! 帧缓冲控制台，并执行清屏、退出与按需启动服务等平台副作用。

use alloc::format;
use alloc::string::String;

use hk_hal::{BootInfo, Console};
use hk_platform_x86_64::{FrameAllocator, FramebufferConsole, PageTable, Serial, keyboard};
use hk_root_server::ServiceTable;
use hk_shell::{Session, ShellState, execute, parse};

use crate::{loader, runtime};

/// 按需装载服务所需的上下文。
pub struct Loader<'a> {
    /// 启动信息，包含 Limine 模块。
    pub info: &'a BootInfo,
    /// direct map 基址。
    pub direct_map_base: u64,
    /// 物理帧分配器。
    pub allocator: &'a mut FrameAllocator,
    /// 内核页表。
    pub paging: &'a PageTable,
}

/// 执行一组命令。
pub fn run(
    console: &Serial,
    screen: Option<&FramebufferConsole>,
    services: &mut ServiceTable,
    loader: &mut Loader<'_>,
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

    let mut session = Session::new();
    for line in script {
        if line.is_empty() {
            continue;
        }
        write(console, screen, &format!("hk> {line}\n"));
        if execute_line(console, screen, services, loader, &mut session, line) {
            break;
        }
    }
    Ok(())
}

/// 运行交互式 HolyShell：从键盘或串口读取命令行。
pub fn interactive(
    console: &Serial,
    screen: Option<&FramebufferConsole>,
    services: &mut ServiceTable,
    loader: &mut Loader<'_>,
) -> Result<(), String> {
    write(console, screen, "HolyShell 交互模式（help 查看命令）\n");

    let mut session = Session::new();
    loop {
        let prompt = match session.current() {
            Some(current) => format!("hk[{current}]> "),
            None => String::from("hk> "),
        };
        write(console, screen, &prompt);
        let line = read_line(console, screen);
        let command = line.trim();
        if command.is_empty() {
            continue;
        }
        if execute_line(console, screen, services, loader, &mut session, command) {
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
    services: &mut ServiceTable,
    loader: &mut Loader<'_>,
    session: &mut Session,
    line: &str,
) -> bool {
    let response = {
        let tasks = runtime::tasks();
        let state = ShellState::new(services, &tasks);
        execute(parse(line), &state, session)
    };

    if response.clear {
        if let Some(screen) = screen {
            screen.clear();
        }
    }
    write(console, screen, &response.output);

    if let Some(name) = &response.start {
        let parent = session
            .current()
            .and_then(runtime::tcb_of)
            .or(Some(runtime::root()));
        let output = start_service(loader, services, name, parent);
        write(console, screen, &output);
    }
    response.exit
}

fn start_service(
    loader: &mut Loader<'_>,
    services: &mut ServiceTable,
    name: &str,
    parent: Option<u64>,
) -> String {
    let running = runtime::tasks()
        .iter()
        .any(|task| task.name == name && task.state != "exited");
    if running {
        return format!("已在运行：{name}\n");
    }

    let slot = runtime::task_count();
    let context = match loader::load_service(
        loader.info,
        name,
        slot,
        loader.direct_map_base,
        loader.allocator,
        loader.paging,
    ) {
        Ok(context) => context,
        Err(error) => return format!("装载失败：{error}\n"),
    };

    if let Err(error) = runtime::start(name, context, parent) {
        return format!("启动失败：{error}\n");
    }

    services.mark_running(name);
    runtime::run(services);
    format!("已启动：{name}\n")
}

fn write(console: &Serial, screen: Option<&FramebufferConsole>, text: &str) {
    console.write_str(text);
    if let Some(screen) = screen {
        screen.write_str(text);
    }
}
