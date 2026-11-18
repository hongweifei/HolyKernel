//! 构建目标。

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::cargo;
use crate::failure::{Failure, Result};
use crate::target::{Selection, Target};
use crate::workspace;

/// 构建选中的目标。
pub fn run(selection: Selection, release: bool) -> Result<()> {
    for target in selection.targets() {
        build_target(*target, release)?;
    }
    Ok(())
}

pub(crate) fn build_target(target: Target, release: bool) -> Result<()> {
    let linker = match target.triple() {
        Some(_) => Some(prepare_linker_script()?),
        None => None,
    };
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
        if let Some(linker) = &linker {
            command.env("RUSTFLAGS", rustflags(linker));
        }
        cargo::run(command)?;
    }
    Ok(())
}

/// 生成共享链接脚本；所有服务与内核平台镜像共用同一镜像区。
fn prepare_linker_script() -> Result<PathBuf> {
    let path = workspace::root().join("build/linker.ld");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| Failure::new(format!("创建 {} 失败：{error}", parent.display())))?;
    }
    hk_server_build::write_linker_script(&path, hk_server_build::IMAGE_BASE)
        .map_err(|error| Failure::new(format!("写入 {} 失败：{error}", path.display())))?;
    Ok(path)
}

/// 在现有 `RUSTFLAGS` 后追加链接参数。
fn rustflags(linker: &Path) -> String {
    let mut flags = env::var("RUSTFLAGS").unwrap_or_default();
    if !flags.is_empty() {
        flags.push(' ');
    }
    flags.push_str(&format!(
        "-C link-arg=-T{} -C link-arg=--build-id=none -C link-arg=-no-pie",
        linker.display()
    ));
    flags
}
