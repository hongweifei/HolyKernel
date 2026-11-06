//! HolyShell 平台适配：串口/帧缓冲 I/O、服务装载与调度回调。
//!
//! 通用命令宿主位于 `hk-kernel-shell`；本模块只提供控制台 I/O、服务装载器与
//! 调度回调。

use alloc::format;
use alloc::string::String;

use hk_hal::{BootInfo, Console};
use hk_kernel_shell::{Scheduling, ScriptSource};
use hk_platform_x86_64::cpu;
use hk_platform_x86_64::{FrameAllocator, FramebufferConsole, PageTable, Serial, keyboard};
use hk_root_server::ServiceTable;
use hk_shell::ShellIo;

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

    fn load_module(&mut self, module: &str, slot: usize) -> Result<cpu::InterruptFrame, String> {
        loader::load_module(
            self.info,
            module,
            slot,
            self.direct_map_base,
            self.allocator,
            self.paging,
        )
    }

    fn has_module(&self, module: &str) -> bool {
        self.info.server_modules.iter().any(|candidate| {
            !hk_root_server::loader::is_script(candidate.name)
                && hk_root_server::loader::service_name(candidate.name) == module
        })
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

    fn priority(&self, name: &str) -> u8 {
        let Some(module) = self
            .info
            .server_modules
            .iter()
            .find(|module| hk_root_server::loader::service_name(module.name) == name)
        else {
            return 16;
        };
        self.info
            .module_bytes(module)
            .map(hk_root_server::loader::service_priority)
            .unwrap_or(16)
    }
}

impl ScriptSource for Loader<'_> {
    fn script(&self, name: &str) -> Result<String, String> {
        let wanted = if name.ends_with(".hsh") {
            String::from(name)
        } else {
            format!("{name}.hsh")
        };
        let module = self
            .info
            .server_modules
            .iter()
            .find(|module| hk_root_server::loader::module_name(module.name) == wanted)
            .ok_or_else(|| format!("未找到脚本：{wanted}"))?;

        let bytes = self
            .info
            .module_bytes(module)
            .ok_or_else(|| format!("脚本不在 direct map 内：{wanted}"))?;
        core::str::from_utf8(bytes)
            .map(String::from)
            .map_err(|_| format!("脚本不是 UTF-8：{wanted}"))
    }
}

/// 平台调度回调。
struct Scheduler {
    /// 嵌套服务运行前的 tick 状态；`None` 表示不在嵌套运行中。
    tick_restore: Option<bool>,
}

impl Scheduler {
    const fn new() -> Self {
        Self { tick_restore: None }
    }
}

impl Scheduling for Scheduler {
    fn run_budgeted(&mut self, services: &mut ServiceTable, steps: usize) -> bool {
        runtime::run_budgeted(services, steps)
    }

    fn begin_service_run(&mut self) {
        if self.tick_restore.is_some() {
            return;
        }
        let armed = cpu::tick_enabled();
        self.tick_restore = Some(armed);
        if !armed {
            cpu::rearm_tick();
        }
    }

    fn end_service_run(&mut self) {
        if let Some(armed) = self.tick_restore.take()
            && !armed
        {
            cpu::stop_tick();
        }
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
    hk_kernel_shell::run(
        &io,
        runtime::host(),
        services,
        loader,
        &mut Scheduler::new(),
        script,
    );
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
    hk_kernel_shell::interactive(
        &io,
        runtime::host(),
        services,
        loader,
        &mut Scheduler::new(),
    );
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
