//! 运行目标。

use crate::cargo;
use crate::failure::{Failure, Result};
use crate::target::Selection;

/// 运行选中的目标。
///
/// 裸机目标的启动镜像在 M2 提供；在此之前 `run` 只接受 Hosted。
pub fn run(selection: Selection, release: bool, arguments: &[String]) -> Result<()> {
    let targets = selection.targets();
    if targets.len() != 1 {
        return Err(Failure::new("run 只接受单个目标"));
    }
    let target = targets[0];
    if !target.is_hosted() {
        return Err(Failure::new(format!(
            "{} 的启动镜像在 M2 提供；当前使用 cargo xtask build --target {} 校验构建",
            target.name(),
            target.name()
        )));
    }

    let mut command = cargo::command();
    command.args(["run", "-p", "hk-hosted"]);
    if release {
        command.arg("--release");
    }
    if !arguments.is_empty() {
        command.arg("--");
        command.args(arguments);
    }
    cargo::run(command)
}
