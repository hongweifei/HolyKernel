//! HolyShell 平台适配：串口/帧缓冲 I/O、脚本读取与服务启动副作用。
//!
//! 命令解析、输出文本与执行循环由 `hk-shell` 提供；本模块只实现 `ShellIo`
//! 与 `ShellHost`，把输入输出接到硬件，把 `use`/`load` 接到启动模块，把
//! `start` 接到镜像装载与运行时。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use hk_hal::{BootInfo, Console};
use hk_platform_x86_64::{FrameAllocator, FramebufferConsole, PageTable, Serial, keyboard};
use hk_root_server::{RuntimeStats, ServiceTable, TaskInfo, TraceEvent};
use hk_shell::{Session, ShellHost, ShellIo};

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

/// 串口与帧缓冲控制台。
struct ConsoleIo<'a> {
    console: &'a Serial,
    screen: Option<&'a FramebufferConsole>,
}

impl ShellIo for ConsoleIo<'_> {
    fn write(&self, text: &str) {
        self.console.write_str(text);
        if let Some(screen) = self.screen {
            screen.write_str(text);
        }
    }

    fn clear(&self) {
        if let Some(screen) = self.screen {
            screen.clear();
        }
    }

    fn read_line(&self) -> String {
        read_line(self.console, self.screen)
    }
}

/// 命令宿主：任务视图、服务表与按需装载。
struct Host<'a, 'b> {
    services: &'a mut ServiceTable,
    loader: &'a mut Loader<'b>,
}

impl ShellHost for Host<'_, '_> {
    fn tasks(&self) -> Vec<TaskInfo> {
        runtime::tasks()
    }

    fn services(&self) -> &ServiceTable {
        self.services
    }

    fn trace(&self, limit: usize) -> Vec<TraceEvent> {
        runtime::trace(limit)
    }

    fn stats(&self) -> RuntimeStats {
        runtime::stats()
    }

    fn start(&mut self, name: &str, parent: Option<&str>) -> String {
        let parent = parent.and_then(runtime::tcb_of).or(Some(runtime::root()));
        start_service(self.loader, self.services, name, parent)
    }

    fn restart(&mut self, name: &str, parent: Option<&str>) -> String {
        let parent = parent.and_then(runtime::tcb_of).or(Some(runtime::root()));
        restart_service(self.loader, self.services, name, parent)
    }

    fn script(&self, name: &str) -> Result<String, String> {
        let wanted = if name.ends_with(".hsh") {
            String::from(name)
        } else {
            format!("{name}.hsh")
        };
        let module = self
            .loader
            .info
            .server_modules
            .iter()
            .find(|module| hk_root_server::loader::module_name(module.name) == wanted)
            .ok_or_else(|| format!("未找到脚本：{wanted}"))?;

        let address = self.loader.direct_map_base + module.region.base.get();
        // SAFETY: 脚本模块由 bootloader 载入并落在 HHDM 映射内，长度来自启动信息。
        let bytes = unsafe {
            core::slice::from_raw_parts(address as *const u8, module.region.len as usize)
        };
        core::str::from_utf8(bytes)
            .map(String::from)
            .map_err(|_| format!("脚本不是 UTF-8：{wanted}"))
    }
}

/// 执行一组命令。
pub fn run(
    console: &Serial,
    screen: Option<&FramebufferConsole>,
    services: &mut ServiceTable,
    loader: &mut Loader<'_>,
    script: &[&str],
) -> Result<(), String> {
    let io = ConsoleIo { console, screen };
    io.write("HolyShell\n");

    if script.is_empty() {
        io.write("当前仅支持脚本化执行：shell-test <命令>...\n");
        return Ok(());
    }

    let mut host = Host { services, loader };
    let mut session = Session::new();
    hk_shell::run_script(&io, &mut host, &mut session, script);
    Ok(())
}

/// 运行交互式 HolyShell：从键盘或串口读取命令行。
pub fn interactive(
    console: &Serial,
    screen: Option<&FramebufferConsole>,
    services: &mut ServiceTable,
    loader: &mut Loader<'_>,
) -> Result<(), String> {
    let io = ConsoleIo { console, screen };
    io.write("HolyShell 交互模式（help 查看命令）\n");

    let mut host = Host { services, loader };
    let mut session = Session::new();
    hk_shell::run_repl(&io, &mut host, &mut session);
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
    launch(loader, services, name, parent, "已启动")
}

fn restart_service(
    loader: &mut Loader<'_>,
    services: &mut ServiceTable,
    name: &str,
    parent: Option<u64>,
) -> String {
    let Some(task) = runtime::tasks().into_iter().find(|task| task.name == name) else {
        return format!("未知服务：{name}\n");
    };
    if task.state != "exited" {
        return format!("仍在运行：{name}\n");
    }
    launch(loader, services, name, parent, "已重启")
}

fn launch(
    loader: &mut Loader<'_>,
    services: &mut ServiceTable,
    name: &str,
    parent: Option<u64>,
    verb: &str,
) -> String {
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
    format!("{verb}：{name}\n")
}

fn write(console: &Serial, screen: Option<&FramebufferConsole>, text: &str) {
    console.write_str(text);
    if let Some(screen) = screen {
        screen.write_str(text);
    }
}
