//! Shell 命令解析。

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
