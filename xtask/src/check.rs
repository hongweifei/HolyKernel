//! 只做编译检查的目标；用于尚未提供启动入口的架构。

use crate::cargo;
use crate::failure::Result;
use crate::target::{Selection, Target};

/// 对选中的目标执行 `cargo check`，不产镜像。
pub fn run(selection: Selection, release: bool) -> Result<()> {
    for target in selection.targets() {
        check_target(*target, release)?;
    }
    Ok(())
}

fn check_target(target: Target, release: bool) -> Result<()> {
    if target.is_hosted() {
        let mut command = cargo::locked();
        command.args(["check", "-p", "hk-image-hosted"]);
        if release {
            command.arg("--release");
        }
        return cargo::run(command);
    }
    let Some(triple) = target.triple() else {
        return Ok(());
    };
    let mut command = cargo::locked();
    command.args(["check", "--target", triple]);
    for package in Target::kernel_packages() {
        command.args(["-p", package]);
    }
    if release {
        command.arg("--release");
    }
    cargo::run(command)
}
