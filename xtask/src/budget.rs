//! 内核代码行数预算。

use std::fs;
use std::path::Path;

use crate::failure::{Failure, Result};
use crate::workspace;

/// 内核代码行数上限。
pub const KERNEL_LINE_BUDGET: usize = 50_000;

/// 计入预算的内核 crate。
pub const KERNEL_CRATES: [&str; 5] = ["hk-hal", "hk-handle", "hk-ipc", "hk-kernel", "hk-mem"];

/// 统计内核代码行数，超出预算时失败。
pub fn run() -> Result<()> {
    let root = workspace::root();
    let mut total = 0;

    println!("内核代码行数预算：{KERNEL_LINE_BUDGET}");
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

    if total > KERNEL_LINE_BUDGET {
        return Err(Failure::new(format!(
            "内核代码行数 {total} 超出预算 {KERNEL_LINE_BUDGET}"
        )));
    }
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
