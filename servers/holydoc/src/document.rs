//! HolyDoc 文档模型：H: 卷文档集、内联指令解析、自动换行布局与历史。

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

/// 欢迎页 DocPath。
pub(crate) const WELCOME: &str = "H:/Welcome";
/// 帮助页 DocPath。
pub(crate) const HELP: &str = "H:/Help";
/// 文档索引 DocPath。
pub(crate) const INDEX: &str = "H:/Index";

/// 没有卷时的内置页面。
const BUILTIN: &[(&str, &str)] = &[
    (WELCOME, include_str!("../../../docs/holy/Welcome.hd")),
    (HELP, include_str!("../../../docs/holy/Help.hd")),
];

/// 一页文档。
#[derive(Clone)]
pub(crate) struct DocEntry {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) body: String,
}

/// 文档集：H: 卷文档与内置页面。
pub(crate) struct DocSet {
    entries: Vec<DocEntry>,
}

impl DocSet {
    /// 创建空文档集。
    pub(crate) fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// 加入一页文档；DocPath 重复时返回 `false`。
    pub(crate) fn add(&mut self, id: String, title: String, body: String) -> bool {
        if self.entries.iter().any(|entry| entry.id == id) {
            return false;
        }
        self.entries.push(DocEntry { id, title, body });
        true
    }

    /// 补上没有卷时使用的内置页面。
    pub(crate) fn add_builtin(&mut self) {
        for (id, body) in BUILTIN {
            if self.contains(id) {
                continue;
            }
            let title = title_of(body).unwrap_or_else(|| doc_name(id));
            self.entries.push(DocEntry {
                id: String::from(*id),
                title,
                body: String::from(*body),
            });
        }
    }

    /// 返回文档集里的全部页面。
    pub(crate) fn entries(&self) -> &[DocEntry] {
        &self.entries
    }

    /// 判断文档集是否包含指定 DocPath。
    fn contains(&self, id: &str) -> bool {
        id == INDEX || self.entries.iter().any(|entry| entry.id == id)
    }

    /// 按 DocPath 取页面；索引按当前集合即时生成。
    pub(crate) fn page(&self, id: &str) -> Option<DocEntry> {
        if id == INDEX {
            return Some(self.index());
        }
        self.entries.iter().find(|entry| entry.id == id).cloned()
    }

    /// 把链接目标解析为文档集内的 DocPath。
    pub(crate) fn resolve(&self, target: &str) -> Option<String> {
        let target = target.trim();
        if target.is_empty() || target.contains("://") {
            return None;
        }
        let path = if target.starts_with("H:") {
            String::from(target)
        } else if let Some(rest) = target.strip_prefix('/') {
            format!("H:/{rest}")
        } else {
            format!("H:/{target}")
        };
        let path = path.trim_end_matches('/');
        let path = if path.is_empty() { "H:" } else { path };
        self.contains(path).then(|| String::from(path))
    }

    /// 生成文档索引页面。
    fn index(&self) -> DocEntry {
        let mut body = String::from(
            "@title(\"文档索引\")\nH: 卷中的文档与集合按 DocPath 列出，链接直接跳转。\n",
        );
        if self.entries.is_empty() {
            body.push_str("\nH: 卷尚未挂载，当前只有内置页面。\n");
        } else {
            body.push_str("\n@heading(2, \"页面\")\n");
            let mut entries: Vec<&DocEntry> = self.entries.iter().collect();
            entries.sort_by(|left, right| left.id.cmp(&right.id));
            for entry in entries {
                body.push_str(&format!("@link(\"{}\", \"{}\")\n", entry.id, entry.title));
            }
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
        for raw in entry.body.lines() {
            let line = raw.trim_end();
            if line.is_empty() {
                view.lines.push(Line {
                    runs: Vec::new(),
                    code: false,
                });
                continue;
            }
            let Some((name, rest)) = call(line) else {
                view.push_inline(set, line, Style::Body, false);
                continue;
            };
            match name {
                "title" => {
                    if let Some(title) = split_args(rest).first() {
                        view.push_inline(set, title, Style::Title, false);
                    }
                }
                "heading" => {
                    if let Some(title) = split_args(rest).last() {
                        view.push_inline(set, title, Style::Heading, false);
                    }
                }
                "link" => {
                    let args = split_args(rest);
                    if let Some(label) = args.get(1) {
                        view.push_link(set, label, args.first().map(String::as_str));
                    }
                }
                "bullet" => {
                    if let Some(text) = split_args(rest).first() {
                        view.push_inline(set, &format!("· {text}"), Style::Bullet, false);
                    }
                }
                "code" => {
                    if let Some(text) = split_args(rest).first() {
                        view.push_wrapped(text, Style::Code, true);
                    }
                }
                "exec" => {
                    for output in exec(set, rest) {
                        view.push_wrapped(&output, Style::Code, true);
                    }
                }
                _ => view.push_inline(set, line, Style::Body, false),
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

    fn push_link(&mut self, set: &DocSet, label: &str, target: Option<&str>) {
        let target = target.and_then(|target| set.resolve(target));
        self.wrap(
            alloc::vec![Run {
                text: String::from(label),
                style: Style::Link,
                target,
            }],
            false,
        );
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

/// 从 `@title(...)` 提取页面标题。
pub(crate) fn title_of(body: &str) -> Option<String> {
    for line in body.lines() {
        if let Some((name, rest)) = call(line.trim())
            && name == "title"
            && let Some(title) = split_args(rest).first()
        {
            return Some(title.clone());
        }
    }
    None
}

/// DocPath 的最后一段，作为缺省标题。
pub(crate) fn doc_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// 拆分 `@指令` 名称与参数文本。
fn call(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix('@')?;
    let end = rest
        .find(|character: char| character == '(' || character.is_whitespace())
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    Some((&rest[..end], rest[end..].trim()))
}

/// 拆分逗号分隔的参数，保留引号内的逗号，去掉成对引号。
fn split_args(text: &str) -> Vec<String> {
    let text = text.trim();
    let text = text
        .strip_prefix('(')
        .and_then(|text| text.strip_suffix(')'))
        .unwrap_or(text);
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for character in text.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        match character {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ',' if !quoted => {
                args.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(character),
        }
    }
    if !current.trim().is_empty() || !args.is_empty() {
        args.push(current.trim().to_string());
    }
    args
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

/// 解析行内链接写法 `[label](target)`。
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
