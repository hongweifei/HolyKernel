//! 文档校验。
//!
//! 生成 rustdoc，并检查 Markdown 相对链接与文档索引完整性。

use std::fs;
use std::path::{Path, PathBuf};

use crate::cargo;
use crate::failure::{Failure, Result};
use crate::workspace;

/// 生成 rustdoc 并校验文档。
pub fn run() -> Result<()> {
    let mut rustdoc = cargo::locked();
    rustdoc.args(["doc", "--workspace", "--no-deps"]);
    cargo::run(rustdoc)?;

    let root = workspace::root();
    let mut problems = Vec::new();
    problems.extend(check_links(&root)?);
    problems.extend(check_index(
        &root.join("docs"),
        &root.join("docs/README.md"),
    )?);
    problems.extend(check_index(
        &root.join("docs/adr"),
        &root.join("docs/adr/README.md"),
    )?);

    if problems.is_empty() {
        Ok(())
    } else {
        Err(Failure::new(format!(
            "文档校验失败：\n{}",
            problems.join("\n")
        )))
    }
}

fn markdown_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_markdown(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_markdown(directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let entries = fs::read_dir(directory)
        .map_err(|error| Failure::new(format!("读取目录 {} 失败：{error}", directory.display())))?;
    for entry in entries {
        let path = entry
            .map_err(|error| Failure::new(format!("读取目录项失败：{error}")))?
            .path();
        if path.is_dir() {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if matches!(
                name,
                "target" | "build" | ".git" | ".commandcode" | "node_modules"
            ) {
                continue;
            }
            collect_markdown(&path, files)?;
        } else if path.extension().is_some_and(|extension| extension == "md") {
            files.push(path);
        }
    }
    Ok(())
}

fn check_links(root: &Path) -> Result<Vec<String>> {
    let mut problems = Vec::new();
    for file in markdown_files(root)? {
        let text = fs::read_to_string(&file)
            .map_err(|error| Failure::new(format!("读取 {} 失败：{error}", file.display())))?;
        let directory = file.parent().expect("文档位于目录内");
        for (index, line) in text.lines().enumerate() {
            for link in inline_links(line).into_iter().chain(reference_link(line)) {
                let Some(path) = local_path(link) else {
                    continue;
                };
                if !directory.join(path).exists() {
                    problems.push(format!(
                        "{}:{} 链接目标不存在：{link}",
                        relative(root, &file),
                        index + 1
                    ));
                }
            }
        }
    }
    Ok(problems)
}

fn check_index(directory: &Path, index: &Path) -> Result<Vec<String>> {
    if !index.exists() {
        return Ok(vec![format!("索引文件缺失：{}", index.display())]);
    }
    let text = fs::read_to_string(index)
        .map_err(|error| Failure::new(format!("读取 {} 失败：{error}", index.display())))?;
    let entries = fs::read_dir(directory)
        .map_err(|error| Failure::new(format!("读取目录 {} 失败：{error}", directory.display())))?;

    let mut problems = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| Failure::new(format!("读取目录项失败：{error}")))?
            .path();
        if !path.is_file() || path.extension().is_none_or(|extension| extension != "md") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("文档文件名是 UTF-8");
        if name == "README.md" {
            continue;
        }
        if !text.contains(name) {
            problems.push(format!("索引 {} 未列出 {name}", index.display()));
        }
    }
    Ok(problems)
}

fn inline_links(line: &str) -> Vec<&str> {
    let mut links = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find("](") {
        let after = &rest[start + 2..];
        let Some(end) = after.find(')') else {
            break;
        };
        links.push(&after[..end]);
        rest = &after[end + 1..];
    }
    links
}

/// 引用式链接定义：`[label]: path "title"`。
fn reference_link(line: &str) -> Option<&str> {
    let line = line.trim_start();
    let rest = line.strip_prefix('[')?;
    let end = rest.find("]:")?;
    let target = rest[end + 2..].trim();
    (!target.is_empty()).then_some(target)
}

fn local_path(link: &str) -> Option<&str> {
    let link = link.trim();
    if link.is_empty()
        || link.starts_with('#')
        || link.starts_with("http://")
        || link.starts_with("https://")
        || link.starts_with("mailto:")
    {
        return None;
    }
    let link = link.split_whitespace().next().unwrap_or(link);
    let link = link.split('#').next().unwrap_or(link);
    if link.is_empty() { None } else { Some(link) }
}

fn relative(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .display()
        .to_string()
}
