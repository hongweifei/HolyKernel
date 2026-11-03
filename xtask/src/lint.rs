//! 格式与静态检查。

use crate::cargo;
use crate::failure::Result;

/// 运行 `rustfmt` 检查与 `clippy`。
pub fn run() -> Result<()> {
    let mut format = cargo::command();
    format.args(["fmt", "--all", "--check"]);
    cargo::run(format)?;

    let mut clippy = cargo::command();
    clippy.args([
        "clippy",
        "--workspace",
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ]);
    cargo::run(clippy)
}
