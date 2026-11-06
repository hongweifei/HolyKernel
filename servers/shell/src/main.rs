//! 用户态 Shell 服务。
//!
//! 只负责命令编排与 Console I/O；任务/服务/启动等特权操作通过 Root Server
//! 控制协议完成，不依赖内核运行时、装载器或服务表实现。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(any(target_os = "none", test))]
extern crate alloc;

#[cfg(any(target_os = "none", test))]
mod startup;
#[cfg(target_os = "none")]
mod tags;

#[cfg(target_os = "none")]
use alloc::format;
#[cfg(target_os = "none")]
use alloc::string::{String, ToString};
#[cfg(target_os = "none")]
use alloc::vec::Vec;

#[cfg(target_os = "none")]
use hk_server_api::console::{CONSOLE_PING, CONSOLE_PONG, Client};
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::root::{
    ROOT_OP_CHECKPOINT, ROOT_OP_HANDLES, ROOT_OP_REPLACE, ROOT_OP_RESTART,
    ROOT_OP_RESTORE_CHECKPOINT, ROOT_OP_SCRIPT, ROOT_OP_SERVICES, ROOT_OP_START, ROOT_OP_STATS,
    ROOT_OP_TASKS, ROOT_OP_TRACE,
};
#[cfg(target_os = "none")]
use hk_server_api::vfs::list_mounts;
#[cfg(target_os = "none")]
use hk_server_api::{exit, fault_subscribe, recv_data_try, send_data, write, write_u64, yield_now};
#[cfg(target_os = "none")]
use hk_shell::{
    Command, RuntimeStats, ServiceInfo, Session, ShellHost, ShellIo, TaskInfo, TraceEvent,
    execute_line, parse,
};

/// Root 控制结果缓冲长度。
#[cfg(target_os = "none")]
const ROOT_BUFFER: usize = 8192;

/// Console 行输入缓冲长度。
#[cfg(target_os = "none")]
const LINE_BUFFER: usize = 256;

/// Console 握手超时次数。
#[cfg(target_os = "none")]
const HANDSHAKE_ATTEMPTS: usize = 4096;

/// 服务堆大小。
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 256 * 1024;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

#[cfg(target_os = "none")]
struct ConsoleIo {
    console: Client,
}

#[cfg(target_os = "none")]
impl ShellIo for ConsoleIo {
    fn write(&self, text: &str) {
        self.console.write(text.as_bytes());
    }

    fn clear(&self) {
        self.console.clear();
    }

    fn read_line(&self) -> String {
        let mut buffer = [0_u8; LINE_BUFFER];
        let length = self.console.read_line(&mut buffer);
        String::from_utf8_lossy(&buffer[..length]).into_owned()
    }
}

#[cfg(target_os = "none")]
struct UserHost {
    endpoint: u64,
}

#[cfg(target_os = "none")]
impl ShellHost for UserHost {
    fn tasks(&self) -> Vec<TaskInfo> {
        Vec::new()
    }

    fn services(&mut self) -> Vec<ServiceInfo> {
        Vec::new()
    }

    fn trace(&self, _limit: usize) -> Vec<TraceEvent> {
        Vec::new()
    }

    fn stats(&self) -> RuntimeStats {
        RuntimeStats::default()
    }

    fn start(&mut self, name: &str, _parent: Option<&str>) -> String {
        root_text(ROOT_OP_START, name)
    }

    fn restart(&mut self, name: &str, _parent: Option<&str>) -> String {
        root_text(ROOT_OP_RESTART, name)
    }

    fn extension(&mut self, name: &str, args: &str) -> Option<String> {
        match name {
            "mounts" => Some(mount_text(self.endpoint)),
            "debug" => {
                pump_faults(self.endpoint);
                Some(hk_debug::fault_text())
            }
            "replace" => Some(root_text(ROOT_OP_REPLACE, args)),
            "checkpoint" => Some(root_text(ROOT_OP_CHECKPOINT, args)),
            "restore-checkpoint" => Some(root_text(ROOT_OP_RESTORE_CHECKPOINT, args)),
            _ => tags::extension(self.endpoint, name, args),
        }
    }

    fn help_extra(&self) -> String {
        String::from(
            "扩展：mounts、debug（最近异常回溯）、replace <服务> <镜像>、checkpoint <任务>、restore-checkpoint <任务>\n标签：tags、tag <路径> <编号>、untag <路径> <编号>、tags-of <路径>、tag-members <编号>、tag-create <名字>、tag-drop <编号>\n",
        )
    }

    fn script(&self, name: &str) -> Result<String, String> {
        let text = root_text(ROOT_OP_SCRIPT, name);
        if text.starts_with("未找到脚本")
            || text.starts_with("脚本不在")
            || text.starts_with("脚本不是")
            || text.starts_with("Root 控制失败")
        {
            Err(text.trim_end().to_string())
        } else {
            Ok(text)
        }
    }
}

#[cfg(target_os = "none")]
fn mount_text(endpoint: u64) -> String {
    let mut buffer = [0_u8; 4096];
    match list_mounts(endpoint, &mut buffer) {
        Ok(length) => {
            let length = length.min(buffer.len());
            if length == 0 {
                String::from("无挂载\n")
            } else {
                String::from_utf8_lossy(&buffer[..length]).into_owned()
            }
        }
        Err(code) => format!("VFS 不可用：{code}\n"),
    }
}

#[cfg(target_os = "none")]
fn root_text(op: u64, argument: &str) -> String {
    let mut buffer = [0_u8; ROOT_BUFFER];
    match hk_server_api::root::call(op, argument, &mut buffer) {
        Ok(length) => String::from_utf8_lossy(&buffer[..length]).into_owned(),
        Err(code) => format!("Root 控制失败：{code}\n"),
    }
}

/// 等待 Console 进入接收循环，避免启动期消息丢失。
#[cfg(target_os = "none")]
fn wait_console(endpoint: u64, console: u64) -> bool {
    let mut buffer = [0_u8; 8];
    for attempt in 0..HANDSHAKE_ATTEMPTS {
        if attempt % 64 == 0 {
            let _ = send_data(console, CONSOLE_PING, &[]);
        }
        let (label, _, _, _) = recv_data_try(endpoint, &mut buffer);
        if label == CONSOLE_PONG {
            while recv_data_try(endpoint, &mut buffer).0 == CONSOLE_PONG {}
            return true;
        }
        let _ = yield_now();
    }
    false
}

#[cfg(target_os = "none")]
fn run_command(io: &ConsoleIo, host: &mut UserHost, session: &mut Session, line: &str) -> bool {
    let command = parse(line);
    match command {
        Command::Tasks => io.write(&root_text(ROOT_OP_TASKS, "")),
        Command::Handles => io.write(&root_text(ROOT_OP_HANDLES, "")),
        Command::Services => io.write(&root_text(ROOT_OP_SERVICES, "")),
        Command::Trace => io.write(&root_text(ROOT_OP_TRACE, "")),
        Command::Stats => io.write(&root_text(ROOT_OP_STATS, "")),
        Command::Start(name) if !name.is_empty() => io.write(&root_text(ROOT_OP_START, name)),
        Command::Restart(name) if !name.is_empty() => io.write(&root_text(ROOT_OP_RESTART, name)),
        _ => return execute_line(io, host, session, line),
    }
    false
}

/// 取走已投递的异常事件，交给用户态调试存储。
#[cfg(target_os = "none")]
fn pump_faults(endpoint: u64) {
    let mut buffer = [0_u8; 2048];
    loop {
        let (label, _word, _sender, length) = recv_data_try(endpoint, &mut buffer);
        if label == 0 {
            break;
        }
        hk_debug::ingest(label, &buffer[..length]);
    }
}

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let Some(console) = Client::discover(endpoint) else {
        write(b"hk-server-shell: console unavailable\n");
        exit(1)
    };
    if !wait_console(endpoint, console.console_endpoint()) {
        write(b"hk-server-shell: console handshake timeout\n");
        exit(1)
    }
    let _ = fault_subscribe(endpoint);

    let io = ConsoleIo { console };
    let mut host = UserHost { endpoint };
    let mut session = Session::new();

    let startup = startup::load(endpoint);
    let mut executed = 0_u64;
    for command in &startup.commands {
        if run_command(&io, &mut host, &mut session, command) {
            exit(0);
        }
        executed += 1;
    }
    let marked = if startup.mark_once {
        startup::mark_once(endpoint)
    } else {
        false
    };
    if executed > 0 || startup.mark_once {
        write(b"hk-server-shell: home ");
        write_u64(executed);
        write(b" once ");
        write_u64(u64::from(marked));
        write(b"\n");
    }

    write(b"hk-server-shell: ready\n");
    io.write("HolyShell（用户态）\n");

    loop {
        pump_faults(endpoint);
        io.write("hk> ");
        let line = io.read_line();
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if run_command(&io, &mut host, &mut session, line) {
            break;
        }
    }
    exit(0)
}

hk_server_api::server_boilerplate!();
