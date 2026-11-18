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

    // 服务二进制只在 `x86_64-unknown-none` 上包含实现；host 构建退化为空壳，
    // Cargo 会把它们的依赖全部报成 unused。host 轮只覆盖库与宿主工具，服务
    // 二进制由下面的目标轮检查。
    let mut clippy = cargo::locked();
    clippy.args(["clippy", "--workspace", "--all-targets"]);
    for service in hk_catalog::SERVERS
        .iter()
        .copied()
        .chain(["hk-image-x86_64-qemu"])
    {
        clippy.args(["--exclude", service]);
    }
    clippy.args(["--", "-D", "warnings"]);
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
