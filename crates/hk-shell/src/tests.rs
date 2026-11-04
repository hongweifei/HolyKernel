//! Shell 单元测试。

use super::*;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

fn services() -> Vec<ServiceInfo> {
    alloc::vec![ServiceInfo {
        name: String::from("hk-server-demo"),
        state: "exited",
        restarts: 0,
        exit_code: Some(0),
    }]
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

fn run(line: &str, services: &[ServiceInfo], tasks: &[TaskInfo]) -> Response {
    let mut session = Session::new();
    execute(parse(line), &ShellState::new(services, tasks), &mut session)
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
    let services = Vec::new();
    let response = run("echo hello  world", &services, &[]);
    assert_eq!(response.output, "hello  world\n");
    assert!(!response.exit);
}

#[test]
fn lists_tasks_with_state_and_pending_call() {
    let services = Vec::new();
    let tasks = tasks();
    let response = run("tasks", &services, &tasks);
    assert!(response.output.contains("任务表（2 项）"));
    assert!(response.output.contains("hk-server-demo"));
    assert!(response.output.contains("hk-server-echo"));
    assert!(response.output.contains("exited"));
    assert!(response.output.contains("recv"));
    assert!(response.output.contains("父"));
}

#[test]
fn focus_selects_current_task() {
    let services = Vec::new();
    let tasks = tasks();
    let state = ShellState::new(&services, &tasks);
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
    let services = Vec::new();
    let response = run("start hk-server-device", &services, &[]);
    assert_eq!(response.start.as_deref(), Some("hk-server-device"));
    assert_eq!(response.output, "");

    let response = run("start", &services, &[]);
    assert_eq!(response.start, None);
    assert_eq!(response.output, "用法：start <服务>\n");
}

#[test]
fn restart_requests_service_launch() {
    let services = Vec::new();
    let response = run("restart hk-server-demo", &services, &[]);
    assert_eq!(response.restart.as_deref(), Some("hk-server-demo"));
    assert_eq!(response.output, "");

    let response = run("restart", &services, &[]);
    assert_eq!(response.restart, None);
    assert_eq!(response.output, "用法：restart <服务>\n");
}

#[test]
fn trace_requests_events() {
    let services = Vec::new();
    let response = run("trace", &services, &[]);
    assert!(response.trace);
}

#[test]
fn stats_requests_statistics() {
    let services = Vec::new();
    let response = run("stats", &services, &[]);
    assert!(response.stats);
}

#[test]
fn lists_handle_counts() {
    let services = Vec::new();
    let tasks = tasks();
    let response = run("handles", &services, &tasks);
    assert!(response.output.contains("Handle 计数（2 项）"));
    assert!(response.output.contains("hk-server-demo"));
    assert!(response.output.contains('5'));
}

#[test]
fn reports_empty_task_table() {
    let services = Vec::new();
    let response = run("tasks", &services, &[]);
    assert_eq!(response.output, "任务表为空\n");
}

#[test]
fn lists_services_with_exit_codes() {
    let services = services();
    let response = run("services", &services, &[]);
    assert!(response.output.contains("服务表（1 项）"));
    assert!(response.output.contains("hk-server-demo"));
    assert!(response.output.contains("exited"));
    assert!(response.output.contains("重启"));
}

#[test]
fn lists_restart_counts() {
    let services = alloc::vec![ServiceInfo {
        name: String::from("clock"),
        state: "running",
        restarts: 1,
        exit_code: None,
    }];
    let response = run("services", &services, &[]);
    assert!(
        response
            .output
            .lines()
            .any(|line| line.contains("clock") && line.split_whitespace().nth(2) == Some("1"))
    );
}

#[test]
fn reports_empty_service_table() {
    let services = Vec::new();
    let response = run("services", &services, &[]);
    assert_eq!(response.output, "服务表为空\n");
}

#[test]
fn clear_and_exit_are_flags() {
    let services = Vec::new();
    let clear = run("clear", &services, &[]);
    assert!(clear.clear);
    assert_eq!(clear.output, "");

    let exit = run("exit", &services, &[]);
    assert!(exit.exit);
    assert_eq!(exit.output, "");
}

#[test]
fn reports_unknown_command() {
    let services = Vec::new();
    let response = run("bogus x", &services, &[]);
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
    services: Vec<ServiceInfo>,
    started: Vec<String>,
    restarted: Vec<String>,
    scripts: Vec<(&'static str, &'static str)>,
}

impl ShellHost for MockHost {
    fn tasks(&self) -> Vec<TaskInfo> {
        alloc::vec::Vec::new()
    }

    fn services(&mut self) -> Vec<ServiceInfo> {
        self.services.clone()
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
        services: Vec::new(),
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
        services: Vec::new(),
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
        services: Vec::new(),
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
        services: Vec::new(),
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
        services: Vec::new(),
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
        services: Vec::new(),
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
        services: Vec::new(),
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
        services: Vec::new(),
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
        services: Vec::new(),
        started: Vec::new(),
        restarted: Vec::new(),
        scripts: alloc::vec![("loop.hsh", "use loop.hsh\n")],
    };
    let mut session = Session::new();

    run_script(&io, &mut host, &mut session, &["use loop.hsh"]);

    assert!(io.output.borrow().contains("脚本嵌套过深"));
}
