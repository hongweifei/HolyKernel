//! 文档模型：内置页面、磁盘文档集、Markdown 子集解析、自动换行布局与历史。

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use hk_server_api::write;

use crate::exec::exec;

/// 窗口宽度。
pub(crate) const WIDTH: usize = 760;
/// 窗口高度。
pub(crate) const HEIGHT: usize = 520;
/// 行高。
pub(crate) const LINE_HEIGHT: usize = 22;
/// 页面留白。
pub(crate) const MARGIN: usize = 24;
/// 内容宽度。
pub(crate) const CONTENT_WIDTH: usize = WIDTH - MARGIN * 2;

/// 欢迎页编号。
pub(crate) const WELCOME: &str = "welcome";
/// 文档索引编号。
pub(crate) const INDEX: &str = "index";
/// 磁盘文档编号前缀。
pub(crate) const DISK_PREFIX: &str = "disk:";

/// 内置页面。
struct Builtin {
    id: &'static str,
    title: &'static str,
    body: &'static str,
}

/// 不依赖磁盘的内置页面。
const BUILTIN: &[Builtin] = &[
    Builtin {
        id: WELCOME,
        title: "欢迎",
        body: r#"# 欢迎使用 HolyKernel
HolyKernel 是句柄型微内核：内核提供线程、地址空间、句柄与 IPC，驱动、文件系统与界面运行在用户态。

## 系统状态
@exec version
@exec time
@exec ticks
@exec docs

## 开始浏览
- [文档索引](index)
- [帮助](help)

## 操作
- d 打开文档索引
- w 返回欢迎页
- j / k 滚动，空格 / b 翻页
- Backspace 返回上一页
- q 或标题栏关闭按钮退出
"#,
    },
    Builtin {
        id: "help",
        title: "帮助",
        body: r#"# 帮助
## 文档浏览器
HolyDoc 从磁盘读取文档：VFS 挂载 ext4 后，它会扫描 /docs 与 /disk/docs 下的 .md 文件，把每个文件作为一页加入索引。

## 链接
文档中的 [链接](index) 指向内置页面或磁盘文档；磁盘文档之间用相对路径互相引用。

## 显示
正文按 8×16 位图字体排版，ASCII 半宽、汉字全宽；代码块使用独立底色。

[返回欢迎页](welcome)
"#,
    },
];

/// 一页文档。
#[derive(Clone)]
pub(crate) struct DocEntry {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) body: String,
}

/// 文档集：内置页面加上从磁盘加载的文档。
pub(crate) struct DocSet {
    entries: Vec<DocEntry>,
}

impl DocSet {
    /// 创建只含内置页面的文档集。
    pub(crate) fn new() -> Self {
        let entries = BUILTIN
            .iter()
            .map(|builtin| DocEntry {
                id: String::from(builtin.id),
                title: String::from(builtin.title),
                body: String::from(builtin.body),
            })
            .collect();
        Self { entries }
    }

    /// 加入一页文档；编号重复时返回 `false`。
    pub(crate) fn add(&mut self, id: String, title: String, body: String) -> bool {
        if self.entries.iter().any(|entry| entry.id == id) {
            return false;
        }
        self.entries.push(DocEntry { id, title, body });
        true
    }

    /// 返回文档集里的全部页面。
    pub(crate) fn entries(&self) -> &[DocEntry] {
        &self.entries
    }

    /// 统计从磁盘加载的页面数。
    pub(crate) fn disk_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.id.starts_with(DISK_PREFIX))
            .count()
    }

    /// 判断文档集是否包含指定编号。
    fn contains(&self, id: &str) -> bool {
        id == INDEX || self.entries.iter().any(|entry| entry.id == id)
    }

    /// 按编号取页面；文档索引按当前集合即时生成。
    pub(crate) fn page(&self, id: &str) -> Option<DocEntry> {
        if id == INDEX {
            return Some(self.index());
        }
        self.entries.iter().find(|entry| entry.id == id).cloned()
    }

    /// 把链接目标解析为文档集内的编号。
    pub(crate) fn resolve(&self, target: &str) -> Option<String> {
        let target = target.trim();
        if target.is_empty() || target.contains("://") {
            return None;
        }
        if self.contains(target) {
            return Some(String::from(target));
        }
        let file = target.rsplit('/').next().unwrap_or(target);
        let stem = file.strip_suffix(".md").unwrap_or(file);
        if self.contains(stem) {
            return Some(String::from(stem));
        }
        let disk = format!("{DISK_PREFIX}{stem}");
        if self.contains(&disk) {
            return Some(disk);
        }
        None
    }

    /// 生成文档索引页面。
    fn index(&self) -> DocEntry {
        let mut body = String::from(
            "# 文档索引\nHolyDoc 把内置页面与磁盘文档放在同一份索引里，链接可直接跳转。\n\n## 内置页面\n",
        );
        for builtin in BUILTIN {
            body.push_str(&format!("- [{}]({})\n", builtin.title, builtin.id));
        }
        body.push_str("\n## 磁盘文档\n");
        let mut disk = 0;
        for entry in &self.entries {
            if entry.id.starts_with(DISK_PREFIX) {
                body.push_str(&format!("- [{}]({})\n", entry.title, entry.id));
                disk += 1;
            }
        }
        if disk == 0 {
            body.push_str(
                "未找到磁盘文档。挂载 ext4 磁盘后，把 .md 文件放到 /docs 或 /disk/docs 再重新打开。\n",
            );
        }
        DocEntry {
            id: String::from(INDEX),
            title: String::from("文档索引"),
            body,
        }
    }
}

/// 文本样式。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Style {
    Title,
    Heading,
    Body,
    Bullet,
    Code,
    Link,
}

/// 一段同样式文本。
pub(crate) struct Run {
    pub(crate) text: String,
    pub(crate) style: Style,
    pub(crate) target: Option<String>,
}

/// 布局后的一行。
pub(crate) struct Line {
    pub(crate) runs: Vec<Run>,
    pub(crate) code: bool,
}

/// 文档视图。
pub(crate) struct DocView {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) lines: Vec<Line>,
    pub(crate) scroll: usize,
    pub(crate) history: Vec<String>,
}

impl DocView {
    /// 从文档集加载一页并布局。
    pub(crate) fn load(set: &DocSet, id: &str, history: Vec<String>) -> Option<Self> {
        let entry = set.page(id)?;
        let mut view = Self {
            id: entry.id,
            title: entry.title,
            lines: Vec::new(),
            scroll: 0,
            history,
        };
        let mut code = false;
        for raw in entry.body.lines() {
            if raw.trim_start().starts_with("```") {
                code = !code;
                continue;
            }
            if code {
                view.push_wrapped(raw, Style::Code, true);
                continue;
            }
            let line = raw.trim_end();
            if line.is_empty() {
                view.lines.push(Line {
                    runs: Vec::new(),
                    code: false,
                });
            } else if let Some(rest) = line.strip_prefix("# ") {
                view.push_inline(set, rest, Style::Title, false);
            } else if let Some(rest) = line.strip_prefix("## ") {
                view.push_inline(set, rest, Style::Heading, false);
            } else if let Some(rest) = line.strip_prefix("- ") {
                view.push_inline(set, &format!("· {rest}"), Style::Bullet, false);
            } else if let Some(rest) = line.strip_prefix("@exec ") {
                for output in exec(set, rest) {
                    view.push_wrapped(&output, Style::Code, true);
                }
            } else {
                view.push_inline(set, line, Style::Body, false);
            }
        }
        if view.lines.is_empty() {
            view.lines.push(Line {
                runs: Vec::new(),
                code: false,
            });
        }
        Some(view)
    }

    fn push_inline(&mut self, set: &DocSet, text: &str, style: Style, code: bool) {
        self.wrap(parse_inline(set, text, style), code);
    }

    fn push_wrapped(&mut self, text: &str, style: Style, code: bool) {
        self.wrap(
            alloc::vec![Run {
                text: String::from(text),
                style,
                target: None,
            }],
            code,
        );
    }

    fn wrap(&mut self, runs: Vec<Run>, code: bool) {
        let mut current: Vec<Run> = Vec::new();
        let mut x = 0;
        for run in runs {
            for character in run.text.chars() {
                let width = char_width(character);
                if x + width > CONTENT_WIDTH && x > 0 {
                    self.lines.push(Line {
                        runs: core::mem::take(&mut current),
                        code,
                    });
                    x = 0;
                }
                append_run(&mut current, character, run.style, run.target.clone());
                x += width;
            }
        }
        if !current.is_empty() {
            self.lines.push(Line {
                runs: current,
                code,
            });
        }
    }

    pub(crate) fn max_scroll(&self) -> usize {
        (self.lines.len() * LINE_HEIGHT + MARGIN * 2).saturating_sub(HEIGHT)
    }

    pub(crate) fn scroll_by(&mut self, lines: isize) {
        let next = self.scroll as isize + lines * LINE_HEIGHT as isize;
        self.scroll = next.clamp(0, self.max_scroll() as isize) as usize;
    }

    /// 打开链接目标；失败时保持当前页。
    pub(crate) fn open(&mut self, set: &DocSet, target: &str) -> bool {
        let Some(id) = set.resolve(target) else {
            return false;
        };
        if id == self.id {
            return false;
        }
        let Some(entry) = set.page(&id) else {
            return false;
        };
        self.history.push(self.id.clone());
        let history = core::mem::take(&mut self.history);
        match Self::load(set, &entry.id, history) {
            Some(view) => {
                *self = view;
                write(b"hk-server-holydoc: open ");
                write(self.id.as_bytes());
                write(b"\n");
                true
            }
            None => false,
        }
    }

    /// 返回上一页。
    pub(crate) fn back(&mut self, set: &DocSet) -> bool {
        let Some(previous) = self.history.pop() else {
            return false;
        };
        let history = core::mem::take(&mut self.history);
        match Self::load(set, &previous, history) {
            Some(view) => {
                *self = view;
                write(b"hk-server-holydoc: back ");
                write(self.id.as_bytes());
                write(b"\n");
                true
            }
            None => false,
        }
    }

    pub(crate) fn link_at(&self, x: u32, y: u32) -> Option<String> {
        let adjusted = y as isize + self.scroll as isize - MARGIN as isize;
        if adjusted < 0 {
            return None;
        }
        let index = adjusted as usize / LINE_HEIGHT;
        let line = self.lines.get(index)?;
        let mut pen = 0;
        for run in &line.runs {
            for character in run.text.chars() {
                let width = char_width(character);
                if let Some(target) = &run.target
                    && x as usize >= pen
                    && (x as usize) < pen + width
                {
                    return Some(target.clone());
                }
                pen += width;
            }
        }
        None
    }

    pub(crate) fn first_link(&self) -> Option<(u32, u32, String)> {
        let mut y = MARGIN;
        for line in &self.lines {
            let mut pen = 0;
            for run in &line.runs {
                for character in run.text.chars() {
                    if let Some(target) = &run.target {
                        return Some(((pen + 4) as u32, (y + 8) as u32, target.clone()));
                    }
                    pen += char_width(character);
                }
            }
            y += LINE_HEIGHT;
        }
        None
    }
}

/// 判断字符宽度（8×16 字体，非 ASCII 视为双宽）。
pub(crate) fn char_width(character: char) -> usize {
    if u32::from(character) > 0x7F { 16 } else { 8 }
}

/// 从正文首个一级标题取页面标题。
pub(crate) fn title_of(body: &str) -> Option<String> {
    body.lines().find_map(|line| {
        line.strip_prefix("# ")
            .map(|title| title.trim().to_string())
    })
}

/// 向当前行追加一个字符，合并同样式片段。
fn append_run(runs: &mut Vec<Run>, character: char, style: Style, target: Option<String>) {
    if let Some(last) = runs.last_mut()
        && last.style == style
        && last.target == target
    {
        last.text.push(character);
        return;
    }
    let mut text = String::new();
    text.push(character);
    runs.push(Run {
        text,
        style,
        target,
    });
}

/// 解析行内链接。
fn parse_inline(set: &DocSet, text: &str, style: Style) -> Vec<Run> {
    let mut runs = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let Some(close_rel) = rest[open..].find("](") else {
            break;
        };
        let close = open + close_rel;
        let Some(end_rel) = rest[close + 2..].find(')') else {
            break;
        };
        let end = close + 2 + end_rel;
        if open > 0 {
            runs.push(Run {
                text: String::from(&rest[..open]),
                style,
                target: None,
            });
        }
        let label = &rest[open + 1..close];
        let target = set.resolve(&rest[close + 2..end]);
        runs.push(Run {
            text: String::from(label),
            style: Style::Link,
            target,
        });
        rest = &rest[end + 1..];
    }
    if !rest.is_empty() {
        runs.push(Run {
            text: String::from(rest),
            style,
            target: None,
        });
    }
    runs
}
