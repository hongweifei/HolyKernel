//! 运行测试。

use crate::cargo;
use crate::failure::{Failure, Result};
use crate::target::Selection;

/// 在 Hosted 目标上运行工作区测试。
///
/// 裸机目标的测试需要 QEMU 启动路径，由 M2 起的 `qemu-smoke` 任务承担。
pub fn run(selection: Selection) -> Result<()> {
    let targets = selection.targets();
    if targets.len() != 1 || !targets[0].is_hosted() {
        return Err(Failure::new("test 只接受 hosted；裸机测试由 QEMU 任务承担"));
    }

    let mut command = cargo::command();
    command.args(["test", "--workspace"]);
    cargo::run(command)
}
