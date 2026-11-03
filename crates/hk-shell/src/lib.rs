//! HolyShell 命令解析与输出。
//!
//! 本 crate 只把命令行转换成命令与响应文本，不接触硬件；平台负责输入、输出与
//! 执行副作用（清屏、退出）。

#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::String;

use hk_root_server::ServiceTable;

/// 解析后的命令。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command<'a> {
    /// 空行。
    Empty,
    /// 列出命令。
    Help,
    /// 列出服务。
    Services,
    /// 列出挂载点。
    Mounts,
    /// 清屏。
    Clear,
    /// 回显文本。
    Echo(&'a str),
    /// 退出。
    Exit,
    /// 未知命令。
    Unknown(&'a str),
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
}

impl Response {
    const fn empty() -> Self {
        Self {
            output: String::new(),
            clear: false,
            exit: false,
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
        "services" => Command::Services,
        "mounts" => Command::Mounts,
        "clear" => Command::Clear,
        "echo" => Command::Echo(arguments),
        "exit" => Command::Exit,
        other => Command::Unknown(other),
    }
}

/// 执行命令并返回输出与副作用。
pub fn execute(command: Command<'_>, services: &ServiceTable) -> Response {
    match command {
        Command::Empty => Response::empty(),
        Command::Help => Response {
            output: String::from("命令：help、services、mounts、clear、echo <文本>、exit\n"),
            clear: false,
            exit: false,
        },
        Command::Services => Response {
            output: format_services(services),
            clear: false,
            exit: false,
        },
        Command::Mounts => Response {
            output: String::from("尚无挂载（VFS 在 M4 接入）\n"),
            clear: false,
            exit: false,
        },
        Command::Clear => Response {
            clear: true,
            ..Response::empty()
        },
        Command::Echo(text) => Response {
            output: format!("{text}\n"),
            clear: false,
            exit: false,
        },
        Command::Exit => Response {
            exit: true,
            ..Response::empty()
        },
        Command::Unknown(name) => Response {
            output: format!("未知命令：{name}\n"),
            clear: false,
            exit: false,
        },
    }
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

    #[test]
    fn parses_known_commands() {
        assert_eq!(parse(""), Command::Empty);
        assert_eq!(parse("help"), Command::Help);
        assert_eq!(parse("services"), Command::Services);
        assert_eq!(parse("mounts"), Command::Mounts);
        assert_eq!(parse("clear"), Command::Clear);
        assert_eq!(parse("exit"), Command::Exit);
        assert_eq!(parse("echo hello world"), Command::Echo("hello world"));
        assert_eq!(parse("bogus"), Command::Unknown("bogus"));
    }

    #[test]
    fn echoes_arguments() {
        let response = execute(parse("echo hello  world"), &ServiceTable::new());
        assert_eq!(response.output, "hello  world\n");
        assert!(!response.exit);
    }

    #[test]
    fn lists_services_with_exit_codes() {
        let response = execute(parse("services"), &services());
        assert!(response.output.contains("服务表（1 项）"));
        assert!(response.output.contains("hk-server-demo"));
        assert!(response.output.contains("exited"));
    }

    #[test]
    fn reports_empty_service_table() {
        let response = execute(parse("services"), &ServiceTable::new());
        assert_eq!(response.output, "服务表为空\n");
    }

    #[test]
    fn clear_and_exit_are_flags() {
        let clear = execute(parse("clear"), &ServiceTable::new());
        assert!(clear.clear);
        assert_eq!(clear.output, "");

        let exit = execute(parse("exit"), &ServiceTable::new());
        assert!(exit.exit);
        assert_eq!(exit.output, "");
    }

    #[test]
    fn reports_unknown_command() {
        let response = execute(parse("bogus x"), &ServiceTable::new());
        assert_eq!(response.output, "未知命令：bogus\n");
        assert!(!response.exit);
    }
}
