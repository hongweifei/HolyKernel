//! 构建目标。

use crate::cargo;
use crate::failure::Result;
use crate::target::{Selection, Target};

/// 构建选中的目标。
pub fn run(selection: Selection, release: bool) -> Result<()> {
    for target in selection.targets() {
        build_target(*target, release)?;
    }
    Ok(())
}

pub(crate) fn build_target(target: Target, release: bool) -> Result<()> {
    for group in target.package_groups() {
        let mut command = cargo::locked();
        command.arg("build");
        if let Some(triple) = target.triple() {
            command.arg("--target").arg(triple);
        }
        for package in group {
            command.args(["-p", package]);
        }
        if release {
            command.arg("--release");
        }
        cargo::run(command)?;
    }
    Ok(())
}
