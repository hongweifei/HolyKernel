//! HolyShell 命令解析与输出。
//!
//! 本 crate 只把命令行转换成命令与响应文本，不接触硬件；平台负责输入、输出与
//! 执行副作用（清屏、退出、装载服务）。

#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::String;

use hk_root_server::{ServiceTable, TaskInfo};

/// 解析后的命令。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command<'a> {
    /// 空行。
    Empty,
    /// 列出命令。
    Help,
    /// 列出任务。
    Tasks,
    /// 列出任务的 Handle 计数。
    Handles,
    /// 列出服务。
    Services,
    /// 列出挂载点。
    Mounts,
    /// 选择当前任务。
    Focus(&'a str),
    /// 装载并启动服务。
    Start(&'a str),
    /// 清屏。
    Clear,
    /// 回显文本。
    Echo(&'a str),
    /// 退出。
    Exit,
    /// 未知命令。
    Unknown(&'a str),
}

/// 命令执行所需的系统视图。
#[derive(Clone, Copy)]
pub struct ShellState<'a> {
    /// 服务表。
    pub services: &'a ServiceTable,
    /// 任务视图。
    pub tasks: &'a [TaskInfo],
}

impl<'a> ShellState<'a> {
    /// 以服务表与任务视图构造上下文。
    pub const fn new(services: &'a ServiceTable, tasks: &'a [TaskInfo]) -> Self {
        Self { services, tasks }
    }
}

/// Shell 会话：当前任务的选择。
#[derive(Default)]
pub struct Session {
    current: Option<String>,
}

impl Session {
    /// 构造空会话。
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回当前任务名称。
    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }
}

/// 命令执行结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    /// 需要输出的文本。
    pub output: String,
    /// 是否清屏。
    pub clear: bool,
    /// 是否退出。
    pub exit: bool,
    /// 需要装载并启动的服务。
    pub start: Option<String>,
}

impl Response {
    const fn empty() -> Self {
        Self {
            output: String::new(),
            clear: false,
            exit: false,
            start: None,
        }
    }

    fn output(text: String) -> Self {
        Self {
            output: text,
            ..Self::empty()
        }
    }

    fn clear() -> Self {
        Self {
            clear: true,
            ..Self::empty()
        }
    }

    fn exit() -> Self {
        Self {
            exit: true,
            ..Self::empty()
        }
    }
}

/// 解析命令行。
pub fn parse(line: &str) -> Command<'_> {
    let mut parts = line.split_whitespace();
    let Some(name) = parts.next() else {
        return Command::Empty;
    };
    let arguments = line[name.len()..].trim();

    match name {
        "help" => Command::Help,
        "tasks" => Command::Tasks,
        "handles" => Command::Handles,
        "services" => Command::Services,
        "mounts" => Command::Mounts,
        "focus" => Command::Focus(arguments),
        "start" => Command::Start(arguments),
        "clear" => Command::Clear,
        "echo" => Command::Echo(arguments),
        "exit" => Command::Exit,
        other => Command::Unknown(other),
    }
}

/// 执行命令并返回输出与副作用。
pub fn execute(command: Command<'_>, state: &ShellState<'_>, session: &mut Session) -> Response {
    match command {
        Command::Empty => Response::empty(),
        Command::Help => Response::output(String::from(
            "命令：help、tasks、handles、services、mounts、focus [任务]、start <服务>、clear、echo <文本>、exit\n",
        )),
        Command::Tasks => Response::output(format_tasks(state.tasks, session.current())),
        Command::Handles => Response::output(format_handles(state.tasks)),
        Command::Services => Response::output(format_services(state.services)),
        Command::Mounts => Response::output(String::from("尚无挂载（VFS 在 M4 接入）\n")),
        Command::Focus(name) => focus_task(name, state, session),
        Command::Start("") => Response::output(String::from("用法：start <服务>\n")),
        Command::Start(name) => Response {
            start: Some(String::from(name)),
            ..Response::empty()
        },
        Command::Clear => Response::clear(),
        Command::Echo(text) => Response::output(format!("{text}\n")),
        Command::Exit => Response::exit(),
        Command::Unknown(name) => Response::output(format!("未知命令：{name}\n")),
    }
}

fn focus_task(name: &str, state: &ShellState<'_>, session: &mut Session) -> Response {
    if name.is_empty() {
        let output = match session.current() {
            Some(current) => format!("当前任务：{current}\n"),
            None => String::from("当前任务：无\n"),
        };
        return Response::output(output);
    }

    if !state.tasks.iter().any(|task| task.name == name) {
        return Response::output(format!("未知任务：{name}\n"));
    }

    session.current = Some(String::from(name));
    Response::output(format!("当前任务：{name}\n"))
}

fn format_tasks(tasks: &[TaskInfo], current: Option<&str>) -> String {
    if tasks.is_empty() {
        return String::from("任务表为空\n");
    }

    let mut output = format!("任务表（{} 项）\n", tasks.len());
    output.push_str(
        "TCB  名称              父   端点  状态      挂起   退出码  句柄  Arena     当前\n",
    );
    for task in tasks {
        let exit = match task.exit_code {
            Some(code) => format!("{code}"),
            None => String::from("-"),
        };
        let parent = match task.parent {
            Some(id) => format!("{id}"),
            None => String::from("-"),
        };
        let arena = format!("{}/{}K", task.arena_used / 1024, task.arena_size / 1024);
        let marker = if current == Some(task.name.as_str()) {
            "*"
        } else {
            ""
        };
        output.push_str(&format!(
            "{:<4} {:<16} {:<4} {:<4} {:<8} {:<5} {:<6} {:<4} {:<9} {}\n",
            task.tcb,
            task.name,
            parent,
            task.endpoint,
            task.state,
            task.pending,
            exit,
            task.handles,
            arena,
            marker
        ));
    }
    output
}

fn format_handles(tasks: &[TaskInfo]) -> String {
    if tasks.is_empty() {
        return String::from("任务表为空\n");
    }

    let mut output = format!("Handle 计数（{} 项）\n", tasks.len());
    output.push_str("TCB  名称              句柄\n");
    for task in tasks {
        output.push_str(&format!(
            "{:<4} {:<16} {}\n",
            task.tcb, task.name, task.handles
        ));
    }
    output
}

fn format_services(services: &ServiceTable) -> String {
    if services.is_empty() {
        return String::from("服务表为空\n");
    }

    let mut output = format!("服务表（{} 项）\n", services.len());
    output.push_str("名称              状态      退出码\n");
    for service in services.services() {
        let exit = match service.exit_code {
            Some(code) => format!("{code}"),
            None => String::from("-"),
        };
        output.push_str(&format!(
            "{:<16}  {:<8}  {}\n",
            service.name,
            service.state.name(),
            exit
        ));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn services() -> ServiceTable {
        let mut table = ServiceTable::new();
        table.register("hk-server-demo");
        table.mark_exited("hk-server-demo", 0);
        table
    }

    fn tasks() -> alloc::vec::Vec<TaskInfo> {
        alloc::vec![
            TaskInfo {
                tcb: 2,
                name: alloc::string::String::from("hk-server-demo"),
                parent: Some(1),
                endpoint: 1,
                state: "exited",
                pending: "-",
                handles: 5,
                arena_used: 4096,
                arena_size: 65536,
                exit_code: Some(0),
            },
            TaskInfo {
                tcb: 3,
                name: alloc::string::String::from("hk-server-echo"),
                parent: None,
                endpoint: 2,
                state: "blocked",
                pending: "recv",
                handles: 3,
                arena_used: 0,
                arena_size: 65536,
                exit_code: None,
            },
        ]
    }

    fn run(line: &str, table: &ServiceTable, tasks: &[TaskInfo]) -> Response {
        let mut session = Session::new();
        execute(parse(line), &ShellState::new(table, tasks), &mut session)
    }

    #[test]
    fn parses_known_commands() {
        assert_eq!(parse(""), Command::Empty);
        assert_eq!(parse("help"), Command::Help);
        assert_eq!(parse("tasks"), Command::Tasks);
        assert_eq!(parse("handles"), Command::Handles);
        assert_eq!(parse("services"), Command::Services);
        assert_eq!(parse("mounts"), Command::Mounts);
        assert_eq!(parse("focus"), Command::Focus(""));
        assert_eq!(
            parse("focus hk-server-echo"),
            Command::Focus("hk-server-echo")
        );
        assert_eq!(parse("start"), Command::Start(""));
        assert_eq!(
            parse("start hk-server-device"),
            Command::Start("hk-server-device")
        );
        assert_eq!(parse("clear"), Command::Clear);
        assert_eq!(parse("exit"), Command::Exit);
        assert_eq!(parse("echo hello world"), Command::Echo("hello world"));
        assert_eq!(parse("bogus"), Command::Unknown("bogus"));
    }

    #[test]
    fn echoes_arguments() {
        let table = ServiceTable::new();
        let response = run("echo hello  world", &table, &[]);
        assert_eq!(response.output, "hello  world\n");
        assert!(!response.exit);
    }

    #[test]
    fn lists_tasks_with_state_and_pending_call() {
        let table = ServiceTable::new();
        let tasks = tasks();
        let response = run("tasks", &table, &tasks);
        assert!(response.output.contains("任务表（2 项）"));
        assert!(response.output.contains("hk-server-demo"));
        assert!(response.output.contains("hk-server-echo"));
        assert!(response.output.contains("exited"));
        assert!(response.output.contains("recv"));
        assert!(response.output.contains("父"));
    }

    #[test]
    fn focus_selects_current_task() {
        let table = ServiceTable::new();
        let tasks = tasks();
        let state = ShellState::new(&table, &tasks);
        let mut session = Session::new();

        assert_eq!(
            execute(parse("focus"), &state, &mut session).output,
            "当前任务：无\n"
        );
        assert_eq!(
            execute(parse("focus hk-server-echo"), &state, &mut session).output,
            "当前任务：hk-server-echo\n"
        );
        assert_eq!(session.current(), Some("hk-server-echo"));

        let output = execute(parse("tasks"), &state, &mut session).output;
        assert!(
            output
                .lines()
                .any(|line| line.contains("hk-server-echo") && line.contains('*'))
        );

        assert_eq!(
            execute(parse("focus missing"), &state, &mut session).output,
            "未知任务：missing\n"
        );
        assert_eq!(session.current(), Some("hk-server-echo"));
    }

    #[test]
    fn start_requests_service_launch() {
        let table = ServiceTable::new();
        let response = run("start hk-server-device", &table, &[]);
        assert_eq!(response.start.as_deref(), Some("hk-server-device"));
        assert_eq!(response.output, "");

        let response = run("start", &table, &[]);
        assert_eq!(response.start, None);
        assert_eq!(response.output, "用法：start <服务>\n");
    }

    #[test]
    fn lists_handle_counts() {
        let table = ServiceTable::new();
        let tasks = tasks();
        let response = run("handles", &table, &tasks);
        assert!(response.output.contains("Handle 计数（2 项）"));
        assert!(response.output.contains("hk-server-demo"));
        assert!(response.output.contains('5'));
    }

    #[test]
    fn reports_empty_task_table() {
        let table = ServiceTable::new();
        let response = run("tasks", &table, &[]);
        assert_eq!(response.output, "任务表为空\n");
    }

    #[test]
    fn lists_services_with_exit_codes() {
        let table = services();
        let response = run("services", &table, &[]);
        assert!(response.output.contains("服务表（1 项）"));
        assert!(response.output.contains("hk-server-demo"));
        assert!(response.output.contains("exited"));
    }

    #[test]
    fn reports_empty_service_table() {
        let table = ServiceTable::new();
        let response = run("services", &table, &[]);
        assert_eq!(response.output, "服务表为空\n");
    }

    #[test]
    fn clear_and_exit_are_flags() {
        let table = ServiceTable::new();
        let clear = run("clear", &table, &[]);
        assert!(clear.clear);
        assert_eq!(clear.output, "");

        let exit = run("exit", &table, &[]);
        assert!(exit.exit);
        assert_eq!(exit.output, "");
    }

    #[test]
    fn reports_unknown_command() {
        let table = ServiceTable::new();
        let response = run("bogus x", &table, &[]);
        assert_eq!(response.output, "未知命令：bogus\n");
        assert!(!response.exit);
    }
}
