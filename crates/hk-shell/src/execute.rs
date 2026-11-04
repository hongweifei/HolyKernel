//! 命令执行、脚本求值与 REPL。

use alloc::format;
use alloc::string::String;

use crate::command::{Command, parse};
use crate::format::{format_handles, format_services, format_stats, format_tasks, format_trace};
use crate::host::{ShellHost, ShellIo, ShellState};
use crate::response::Response;
use crate::session::Session;

/// 脚本嵌套上限。
const MAX_SCRIPT_DEPTH: usize = 4;

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
