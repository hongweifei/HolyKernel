//! HolyShell 平台适配：串口/帧缓冲 I/O、脚本读取与服务启动副作用。
//!
//! 命令解析、输出文本与执行循环由 `hk-shell` 提供；本模块只实现 `ShellIo`
//! 与 `ShellHost`，把输入输出接到硬件，把 `use`/`load` 接到启动模块，把
//! `start` 接到镜像装载与运行时。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use hk_hal::{BootInfo, Console};
use hk_platform_x86_64::cpu;
use hk_platform_x86_64::{FrameAllocator, FramebufferConsole, PageTable, Serial, keyboard};
use hk_root_server::ServiceTable;
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

impl hk_root_server::service::ServiceLoader<cpu::InterruptFrame> for Loader<'_> {
    fn load(&mut self, name: &str, slot: usize) -> Result<cpu::InterruptFrame, String> {
        loader::load_service(
            self.info,
            name,
            slot,
            self.direct_map_base,
            self.allocator,
            self.paging,
        )
    }

    fn role(&self, name: &str) -> u32 {
        let Some(module) = self
            .info
            .server_modules
            .iter()
            .find(|module| hk_root_server::loader::service_name(module.name) == name)
        else {
            return 0;
        };
        self.info
            .module_bytes(module)
            .map(hk_root_server::loader::service_role)
            .unwrap_or(0)
    }
}

impl Host<'_, '_> {
    /// 启动用户态 `netstat` 客户端并驱动调度，按需查询网络状态。
    ///
    /// 内核态 shell 只负责编排，不接触网络协议或实现。
    fn net_status(&mut self) -> String {
        let message = runtime::start_service(self.loader, self.services, "hk-server-netstat", None);
        if message.contains("失败") || message.contains("未知") {
            return message;
        }
        for _ in 0..64 {
            if runtime::run_budgeted(self.services, 4096) {
                break;
            }
        }
        message
    }
}

impl ShellHost for Host<'_, '_> {
    fn tasks(&self) -> Vec<hk_shell::TaskInfo> {
        runtime::tasks()
            .into_iter()
            .map(|task| hk_shell::TaskInfo {
                tcb: task.tcb,
                name: task.name,
                parent: task.parent,
                endpoint: task.endpoint,
                state: task.state,
                pending: task.pending,
                handles: task.handles,
                arena_used: task.arena_used,
                arena_size: task.arena_size,
                vspace: task.vspace,
                exit_code: task.exit_code,
            })
            .collect()
    }

    fn services(&mut self) -> Vec<hk_shell::ServiceInfo> {
        runtime::drain_exits(self.services);
        self.services
            .services()
            .iter()
            .map(|service| hk_shell::ServiceInfo {
                name: service.name.clone(),
                state: service.state.name(),
                restarts: service.restarts,
                exit_code: service.exit_code,
            })
            .collect()
    }

    fn trace(&self, limit: usize) -> Vec<hk_shell::TraceEvent> {
        runtime::trace(limit)
            .into_iter()
            .map(|event| hk_shell::TraceEvent {
                sequence: event.sequence,
                category: event.category.name(),
                thread: event.thread,
                code: event.code,
                argument: event.argument,
            })
            .collect()
    }

    fn stats(&self) -> hk_shell::RuntimeStats {
        let stats = runtime::stats();
        hk_shell::RuntimeStats {
            switches: stats.switches,
            switch_cycles: stats.switch_cycles,
            switch_max_cycles: stats.switch_max_cycles,
            calls: stats.calls,
            call_cycles: stats.call_cycles,
            call_max_cycles: stats.call_max_cycles,
            resumes: stats.resumes.to_vec(),
            tsc_frequency: stats.tsc_frequency,
        }
    }

    fn extension(&mut self, name: &str, _args: &str) -> Option<String> {
        match name {
            "net" => Some(self.net_status()),
            _ => None,
        }
    }

    fn help_extra(&self) -> String {
        String::from("扩展：net\n")
    }

    fn start(&mut self, name: &str, parent: Option<&str>) -> String {
        runtime::start_service(self.loader, self.services, name, parent)
    }

    fn restart(&mut self, name: &str, parent: Option<&str>) -> String {
        runtime::restart_service(self.loader, self.services, name, parent)
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

        let bytes = self
            .loader
            .info
            .module_bytes(module)
            .ok_or_else(|| format!("脚本不在 direct map 内：{wanted}"))?;
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
        // 串口接收中断未启用，空闲后定时器也可能停止；这里主动轮询，
        // 避免 `hlt` 永久等待而无法读取 stdin。轮询期间保持中断开启，
        // 让 PS/2 键盘与定时器仍能投递。
        // SAFETY: 仅设置中断标志，不改变栈或寄存器状态。
        unsafe {
            core::arch::asm!("sti", options(nomem, nostack));
        }
        core::hint::spin_loop();
    }
}

fn write(console: &Serial, screen: Option<&FramebufferConsole>, text: &str) {
    console.write_str(text);
    if let Some(screen) = screen {
        screen.write_str(text);
    }
}
