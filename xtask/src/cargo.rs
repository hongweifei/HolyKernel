//! cargo 命令调用。

use std::env;
use std::process::Command;

use crate::failure::{Failure, Result};
use crate::workspace;

/// 构造 cargo 命令，工作目录固定为仓库根。
pub fn command() -> Command {
    let mut command = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command.current_dir(workspace::root());
    command
}

/// 构造带 `--locked` 的 cargo 命令；用于构建、测试与文档。
pub fn locked() -> Command {
    let mut command = command();
    command.arg("--locked");
    command
}

/// 运行命令；失败时把命令行附在错误里。
pub fn run(mut command: Command) -> Result<()> {
    let description = format!("{command:?}");
    let status = command
        .status()
        .map_err(|error| Failure::new(format!("无法启动 {description}：{error}")))?;
    if !status.success() {
        return Err(Failure::new(format!("命令失败（{status}）：{description}")));
    }
    Ok(())
}
