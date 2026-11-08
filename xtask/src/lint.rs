//! 格式与静态检查。

use crate::cargo;
use crate::failure::Result;
use crate::target::Target;

/// 运行 `rustfmt` 检查与 `clippy`。
pub fn run() -> Result<()> {
    crate::deps::run()?;

    let mut format = cargo::command();
    format.args(["fmt", "--all", "--check"]);
    cargo::run(format)?;

    let mut clippy = cargo::locked();
    clippy.args([
        "clippy",
        "--workspace",
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ]);
    cargo::run(clippy)?;

    let target = Target::X86_64Qemu;
    let triple = target.triple().expect("x86_64-qemu 有目标三元组");
    for group in target.package_groups() {
        let mut clippy = cargo::locked();
        clippy.args(["clippy", "--target", triple]);
        for package in group {
            clippy.args(["-p", package]);
        }
        clippy.args(["--", "-D", "warnings"]);
        cargo::run(clippy)?;
    }
    Ok(())
}
