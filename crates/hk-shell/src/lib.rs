//! HolyShell 命令解析与输出。
//!
//! 本 crate 只把命令行转换成命令与响应文本，不接触硬件；平台负责输入、输出与
//! 执行副作用（清屏、退出、装载服务）。

#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use hk_root_server::{RuntimeStats, ServiceTable, TaskInfo, TraceEvent};

/// 脚本嵌套上限。
const MAX_SCRIPT_DEPTH: usize = 4;

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
    /// 列出最近的内核 trace 事件。
    Trace,
    /// 显示运行时统计。
    Stats,
    /// 选择当前任务。
    Focus(&'a str),
    /// 装载并启动服务。
    Start(&'a str),
    /// 重启已退出的服务。
    Restart(&'a str),
    /// 求值 HolyShell 脚本。
    Use(&'a str),
    /// 求值 HolyShell 脚本（`use` 的别名）。
    Load(&'a str),
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

/// Shell 会话：当前任务的选择与脚本嵌套深度。
#[derive(Default)]
pub struct Session {
    current: Option<String>,
    depth: usize,
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

/// Shell 的输入与输出。
pub trait ShellIo {
    /// 写出文本。
    fn write(&self, text: &str);

    /// 清屏。
    fn clear(&self);

    /// 读取一行输入，不包含换行符。
    fn read_line(&self) -> String;
}

/// Shell 命令宿主：任务视图与服务启停。
pub trait ShellHost {
    /// 返回任务视图快照。
    fn tasks(&self) -> Vec<TaskInfo>;

    /// 返回服务表。
    fn services(&self) -> &ServiceTable;

    /// 返回最近的内核 trace 事件。
    fn trace(&self, limit: usize) -> Vec<TraceEvent>;

    /// 返回运行时统计。
    fn stats(&self) -> RuntimeStats;

    /// 启动服务；`parent` 是当前会话选中的任务名。
    fn start(&mut self, name: &str, parent: Option<&str>) -> String;

    /// 重启已退出的服务；`parent` 是当前会话选中的任务名。
    fn restart(&mut self, name: &str, parent: Option<&str>) -> String;

    /// 读取脚本内容；名称可省略 `.hsh` 后缀。
    fn script(&self, name: &str) -> Result<String, String>;
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
    /// 需要重启的服务。
    pub restart: Option<String>,
    /// 需要求值的脚本。
    pub script: Option<String>,
    /// 是否需要输出 trace。
    pub trace: bool,
    /// 是否需要输出统计。
    pub stats: bool,
}

impl Response {
    const fn empty() -> Self {
        Self {
            output: String::new(),
            clear: false,
            exit: false,
            start: None,
            restart: None,
            script: None,
            trace: false,
            stats: false,
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
        "trace" => Command::Trace,
        "stats" => Command::Stats,
        "focus" => Command::Focus(arguments),
        "start" => Command::Start(arguments),
        "restart" => Command::Restart(arguments),
        "use" => Command::Use(arguments),
        "load" => Command::Load(arguments),
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
            "命令：help、tasks、handles、services、mounts、trace、stats、focus [任务]、start <服务>、restart <服务>、use <脚本>、load <脚本>、clear、echo <文本>、exit\n",
        )),
        Command::Tasks => Response::output(format_tasks(state.tasks, session.current())),
        Command::Handles => Response::output(format_handles(state.tasks)),
        Command::Services => Response::output(format_services(state.services)),
        Command::Mounts => Response::output(String::from("尚无挂载（VFS 在 M4 接入）\n")),
        Command::Trace => Response {
            trace: true,
            ..Response::empty()
        },
        Command::Stats => Response {
            stats: true,
            ..Response::empty()
        },
        Command::Focus(name) => focus_task(name, state, session),
        Command::Start("") => Response::output(String::from("用法：start <服务>\n")),
        Command::Start(name) => Response {
            start: Some(String::from(name)),
            ..Response::empty()
        },
        Command::Restart("") => Response::output(String::from("用法：restart <服务>\n")),
        Command::Restart(name) => Response {
            restart: Some(String::from(name)),
            ..Response::empty()
        },
        Command::Use("") | Command::Load("") => {
            Response::output(String::from("用法：use <脚本>\n"))
        }
        Command::Use(name) | Command::Load(name) => Response {
            script: Some(String::from(name)),
            ..Response::empty()
        },
        Command::Clear => Response::clear(),
        Command::Echo(text) => Response::output(format!("{text}\n")),
        Command::Exit => Response::exit(),
        Command::Unknown(name) => Response::output(format!("未知命令：{name}\n")),
    }
}

/// 执行一行命令并处理副作用；返回是否退出。
pub fn execute_line<H: ShellHost>(
    io: &impl ShellIo,
    host: &mut H,
    session: &mut Session,
    line: &str,
) -> bool {
    let response = {
        let tasks = host.tasks();
        let state = ShellState::new(host.services(), &tasks);
        execute(parse(line), &state, session)
    };

    if response.clear {
        io.clear();
    }
    io.write(&response.output);

    if let Some(name) = &response.start {
        let output = host.start(name, session.current());
        io.write(&output);
    }

    if let Some(name) = &response.restart {
        let output = host.restart(name, session.current());
        io.write(&output);
    }

    if response.trace {
        let events = host.trace(32);
        io.write(&format_trace(&events));
    }

    if response.stats {
        io.write(&format_stats(&host.stats()));
    }

    if let Some(name) = &response.script {
        return evaluate_script(io, host, session, name);
    }
    response.exit
}

/// 在当前会话中求值脚本；返回是否退出。
fn evaluate_script<H: ShellHost>(
    io: &impl ShellIo,
    host: &mut H,
    session: &mut Session,
    name: &str,
) -> bool {
    if session.depth >= MAX_SCRIPT_DEPTH {
        io.write("脚本嵌套过深\n");
        return false;
    }

    let text = match host.script(name) {
        Ok(text) => text,
        Err(error) => {
            io.write(&format!("无法加载脚本：{error}\n"));
            return false;
        }
    };

    io.write(&format!("求值：{name}\n"));
    session.depth += 1;
    let mut exited = false;
    for line in text.lines() {
        let command = line.trim();
        if command.is_empty() || command.starts_with('#') {
            continue;
        }
        if execute_line(io, host, session, command) {
            exited = true;
            break;
        }
    }
    session.depth -= 1;
    exited
}

/// 交互式执行，直到 `exit` 或输入结束。
pub fn run_repl<H: ShellHost>(io: &impl ShellIo, host: &mut H, session: &mut Session) {
    loop {
        let prompt = match session.current() {
            Some(current) => format!("hk[{current}]> "),
            None => String::from("hk> "),
        };
        io.write(&prompt);

        let line = io.read_line();
        let command = line.trim();
        if command.is_empty() {
            continue;
        }
        if execute_line(io, host, session, command) {
            break;
        }
    }
}

/// 脚本化执行，遇到 `exit` 停止。
pub fn run_script<H: ShellHost>(
    io: &impl ShellIo,
    host: &mut H,
    session: &mut Session,
    script: &[&str],
) {
    for line in script {
        if line.is_empty() {
            continue;
        }
        io.write(&format!("hk> {line}\n"));
        if execute_line(io, host, session, line) {
            break;
        }
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
        "TCB  名称              父   端点  状态      挂起   退出码  句柄  Arena     VSpace       当前\n",
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
        let vspace = format!("{:#x}", task.vspace);
        let marker = if current == Some(task.name.as_str()) {
            "*"
        } else {
            ""
        };
        output.push_str(&format!(
            "{:<4} {:<16} {:<4} {:<4} {:<8} {:<5} {:<6} {:<4} {:<9} {:<12} {}\n",
            task.tcb,
            task.name,
            parent,
            task.endpoint,
            task.state,
            task.pending,
            exit,
            task.handles,
            arena,
            vspace,
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

fn format_trace(events: &[TraceEvent]) -> String {
    if events.is_empty() {
        return String::from("trace 为空\n");
    }

    let mut output = format!("trace（{} 项）\n", events.len());
    output.push_str("序号  类别   线程  代码  参数\n");
    for event in events {
        output.push_str(&format!(
            "{:<4} {:<6} {:<4} {:<4} {:#x}\n",
            event.sequence,
            event.category.name(),
            event.thread,
            event.code,
            event.argument
        ));
    }
    output
}

fn format_stats(stats: &RuntimeStats) -> String {
    let mut output = String::from("运行时统计\n");
    if stats.tsc_frequency == 0 {
        output.push_str("  TSC 频率：未知\n");
        return output;
    }

    output.push_str(&format!(
        "  上下文切换：{} 次，平均 {} ns，最大 {} ns\n",
        stats.switches,
        average_ns(stats.switch_cycles, stats.switches, stats.tsc_frequency),
        ns(stats.switch_max_cycles, stats.tsc_frequency),
    ));
    output.push_str(&format!(
        "  IPC call/reply：{} 次，平均 {} ns，最大 {} ns\n",
        stats.calls,
        average_ns(stats.call_cycles, stats.calls, stats.tsc_frequency),
        ns(stats.call_max_cycles, stats.tsc_frequency),
    ));
    output
}

fn average_ns(cycles: u64, count: u64, frequency: u64) -> u64 {
    if count == 0 || frequency == 0 {
        return 0;
    }
    ((u128::from(cycles) * 1_000_000_000) / (u128::from(frequency) * u128::from(count))) as u64
}

fn ns(cycles: u64, frequency: u64) -> u64 {
    if frequency == 0 {
        return 0;
    }
    ((u128::from(cycles) * 1_000_000_000) / u128::from(frequency)) as u64
}

fn format_services(services: &ServiceTable) -> String {
    if services.is_empty() {
        return String::from("服务表为空\n");
    }

    let mut output = format!("服务表（{} 项）\n", services.len());
    output.push_str("名称              状态      重启  退出码\n");
    for service in services.services() {
        let exit = match service.exit_code {
            Some(code) => format!("{code}"),
            None => String::from("-"),
        };
        output.push_str(&format!(
            "{:<16}  {:<8}  {:<4}  {}\n",
            service.name,
            service.state.name(),
            service.restarts,
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
                vspace: 0x10_0000,
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
                vspace: 0x11_0000,
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
        assert_eq!(parse("trace"), Command::Trace);
        assert_eq!(parse("stats"), Command::Stats);
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
        assert_eq!(parse("restart"), Command::Restart(""));
        assert_eq!(
            parse("restart hk-server-demo"),
            Command::Restart("hk-server-demo")
        );
        assert_eq!(parse("use"), Command::Use(""));
        assert_eq!(parse("use boot.hsh"), Command::Use("boot.hsh"));
        assert_eq!(parse("load boot"), Command::Load("boot"));
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
    fn restart_requests_service_launch() {
        let table = ServiceTable::new();
        let response = run("restart hk-server-demo", &table, &[]);
        assert_eq!(response.restart.as_deref(), Some("hk-server-demo"));
        assert_eq!(response.output, "");

        let response = run("restart", &table, &[]);
        assert_eq!(response.restart, None);
        assert_eq!(response.output, "用法：restart <服务>\n");
    }

    #[test]
    fn trace_requests_events() {
        let table = ServiceTable::new();
        let response = run("trace", &table, &[]);
        assert!(response.trace);
    }

    #[test]
    fn stats_requests_statistics() {
        let table = ServiceTable::new();
        let response = run("stats", &table, &[]);
        assert!(response.stats);
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
        assert!(response.output.contains("重启"));
    }

    #[test]
    fn lists_restart_counts() {
        let mut table = ServiceTable::new();
        table.register("clock");
        table.mark_exited("clock", 1);
        table.mark_running("clock");
        let response = run("services", &table, &[]);
        assert!(
            response
                .output
                .lines()
                .any(|line| line.contains("clock") && line.split_whitespace().nth(2) == Some("1"))
        );
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

    struct MockIo {
        output: core::cell::RefCell<String>,
        cleared: core::cell::Cell<bool>,
    }

    impl ShellIo for MockIo {
        fn write(&self, text: &str) {
            self.output.borrow_mut().push_str(text);
        }

        fn clear(&self) {
            self.cleared.set(true);
        }

        fn read_line(&self) -> String {
            String::new()
        }
    }

    struct MockHost {
        services: ServiceTable,
        started: Vec<String>,
        restarted: Vec<String>,
        scripts: Vec<(&'static str, &'static str)>,
    }

    impl ShellHost for MockHost {
        fn tasks(&self) -> Vec<TaskInfo> {
            alloc::vec::Vec::new()
        }

        fn services(&self) -> &ServiceTable {
            &self.services
        }

        fn start(&mut self, name: &str, _parent: Option<&str>) -> String {
            self.started.push(String::from(name));
            format!("已启动：{name}\n")
        }

        fn restart(&mut self, name: &str, _parent: Option<&str>) -> String {
            self.restarted.push(String::from(name));
            format!("已重启：{name}\n")
        }

        fn trace(&self, _limit: usize) -> Vec<TraceEvent> {
            Vec::new()
        }

        fn stats(&self) -> RuntimeStats {
            RuntimeStats::default()
        }

        fn script(&self, name: &str) -> Result<String, String> {
            self.scripts
                .iter()
                .find(|(script, _)| *script == name)
                .map(|(_, text)| String::from(*text))
                .ok_or_else(|| format!("未找到脚本：{name}"))
        }
    }

    #[test]
    fn script_prints_empty_trace() {
        let io = MockIo {
            output: core::cell::RefCell::new(String::new()),
            cleared: core::cell::Cell::new(false),
        };
        let mut host = MockHost {
            services: ServiceTable::new(),
            started: Vec::new(),
            restarted: Vec::new(),
            scripts: Vec::new(),
        };
        let mut session = Session::new();

        run_script(&io, &mut host, &mut session, &["trace"]);

        assert!(io.output.borrow().contains("trace 为空"));
    }

    #[test]
    fn script_prints_stats() {
        let io = MockIo {
            output: core::cell::RefCell::new(String::new()),
            cleared: core::cell::Cell::new(false),
        };
        let mut host = MockHost {
            services: ServiceTable::new(),
            started: Vec::new(),
            restarted: Vec::new(),
            scripts: Vec::new(),
        };
        let mut session = Session::new();

        run_script(&io, &mut host, &mut session, &["stats"]);

        assert!(io.output.borrow().contains("运行时统计"));
    }

    #[test]
    fn script_runs_until_exit() {
        let io = MockIo {
            output: core::cell::RefCell::new(String::new()),
            cleared: core::cell::Cell::new(false),
        };
        let mut host = MockHost {
            services: ServiceTable::new(),
            started: Vec::new(),
            restarted: Vec::new(),
            scripts: Vec::new(),
        };
        let mut session = Session::new();

        run_script(
            &io,
            &mut host,
            &mut session,
            &["echo hi", "exit", "echo no"],
        );

        let output = io.output.borrow();
        assert!(output.contains("hk> echo hi\n"));
        assert!(output.contains("hi\n"));
        assert!(!output.contains("echo no"));
    }

    #[test]
    fn script_starts_service_through_host() {
        let io = MockIo {
            output: core::cell::RefCell::new(String::new()),
            cleared: core::cell::Cell::new(false),
        };
        let mut host = MockHost {
            services: ServiceTable::new(),
            started: Vec::new(),
            restarted: Vec::new(),
            scripts: Vec::new(),
        };
        let mut session = Session::new();

        run_script(&io, &mut host, &mut session, &["start clock"]);

        assert_eq!(host.started, alloc::vec!["clock"]);
        assert!(io.output.borrow().contains("已启动：clock\n"));
    }

    #[test]
    fn script_restarts_service_through_host() {
        let io = MockIo {
            output: core::cell::RefCell::new(String::new()),
            cleared: core::cell::Cell::new(false),
        };
        let mut host = MockHost {
            services: ServiceTable::new(),
            started: Vec::new(),
            restarted: Vec::new(),
            scripts: Vec::new(),
        };
        let mut session = Session::new();

        run_script(&io, &mut host, &mut session, &["restart clock"]);

        assert_eq!(host.restarted, alloc::vec!["clock"]);
        assert!(io.output.borrow().contains("已重启：clock\n"));
    }

    #[test]
    fn use_evaluates_script_in_session() {
        let io = MockIo {
            output: core::cell::RefCell::new(String::new()),
            cleared: core::cell::Cell::new(false),
        };
        let mut host = MockHost {
            services: ServiceTable::new(),
            started: Vec::new(),
            restarted: Vec::new(),
            scripts: alloc::vec![("boot.hsh", "# 注释\necho script-ok\nstart clock\n")],
        };
        let mut session = Session::new();

        run_script(&io, &mut host, &mut session, &["use boot.hsh"]);

        let output = io.output.borrow();
        assert!(output.contains("求值：boot.hsh\n"));
        assert!(output.contains("script-ok\n"));
        assert_eq!(host.started, alloc::vec!["clock"]);
    }

    #[test]
    fn load_is_alias_for_use() {
        let io = MockIo {
            output: core::cell::RefCell::new(String::new()),
            cleared: core::cell::Cell::new(false),
        };
        let mut host = MockHost {
            services: ServiceTable::new(),
            started: Vec::new(),
            restarted: Vec::new(),
            scripts: alloc::vec![("boot", "echo loaded\n")],
        };
        let mut session = Session::new();

        run_script(&io, &mut host, &mut session, &["load boot"]);

        assert!(io.output.borrow().contains("loaded\n"));
    }

    #[test]
    fn reports_missing_script() {
        let io = MockIo {
            output: core::cell::RefCell::new(String::new()),
            cleared: core::cell::Cell::new(false),
        };
        let mut host = MockHost {
            services: ServiceTable::new(),
            started: Vec::new(),
            restarted: Vec::new(),
            scripts: Vec::new(),
        };
        let mut session = Session::new();

        run_script(&io, &mut host, &mut session, &["use missing"]);

        assert!(
            io.output
                .borrow()
                .contains("无法加载脚本：未找到脚本：missing")
        );
    }

    #[test]
    fn limits_nested_scripts() {
        let io = MockIo {
            output: core::cell::RefCell::new(String::new()),
            cleared: core::cell::Cell::new(false),
        };
        let mut host = MockHost {
            services: ServiceTable::new(),
            started: Vec::new(),
            restarted: Vec::new(),
            scripts: alloc::vec![("loop.hsh", "use loop.hsh\n")],
        };
        let mut session = Session::new();

        run_script(&io, &mut host, &mut session, &["use loop.hsh"]);

        assert!(io.output.borrow().contains("脚本嵌套过深"));
    }
}
