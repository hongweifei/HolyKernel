//! 内核代码行数统计。
//!
//! 统计内核 crate 的代码行数（去掉空行与整行注释），用于跟踪规模；不设上限。

use std::fs;
use std::path::Path;

use crate::failure::{Failure, Result};
use crate::workspace;

/// 计入统计的内核 crate。
pub const KERNEL_CRATES: [&str; 5] = ["hk-hal", "hk-handle", "hk-ipc", "hk-kernel", "hk-mem"];

/// 统计内核代码行数。
pub fn run() -> Result<()> {
    let root = workspace::root();
    let mut total = 0;

    println!("内核代码行数");
    println!("{:<14}{:>8}", "crate", "lines");
    for name in KERNEL_CRATES {
        let source = root.join("crates").join(name).join("src");
        if !source.exists() {
            continue;
        }
        let lines = count_lines(&source)?;
        total += lines;
        println!("{name:<14}{lines:>8}");
    }
    println!("{:<14}{:>8}", "合计", total);

    Ok(())
}

fn count_lines(directory: &Path) -> Result<usize> {
    let mut total = 0;
    let entries = fs::read_dir(directory)
        .map_err(|error| Failure::new(format!("读取目录 {} 失败：{error}", directory.display())))?;
    for entry in entries {
        let path = entry
            .map_err(|error| Failure::new(format!("读取目录项失败：{error}")))?
            .path();
        if path.is_dir() {
            total += count_lines(&path)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let text = fs::read_to_string(&path)
                .map_err(|error| Failure::new(format!("读取 {} 失败：{error}", path.display())))?;
            total += text
                .lines()
                .filter(|line| {
                    let line = line.trim();
                    !line.is_empty() && !line.starts_with("//")
                })
                .count();
        }
    }
    Ok(total)
}
