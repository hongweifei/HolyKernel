//! HolyDoc 文档模型：H: 卷文档集、标签、布局与历史。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use hk_server_api::write;
use lovyangfx_fonts::Font;

use crate::exec::exec;
use crate::markup::{call, char_width, doc_name, parse_inline, split_args, title_of};

/// 窗口宽度。
pub(crate) const WIDTH: usize = 760;
/// 窗口高度。
pub(crate) const HEIGHT: usize = 520;
/// 行高。
pub(crate) const LINE_HEIGHT: usize = 22;
/// 底部状态栏高度。
pub(crate) const FOOTER_HEIGHT: usize = 22;
/// 正文可视高度。
pub(crate) const CONTENT_HEIGHT: usize = HEIGHT - FOOTER_HEIGHT;
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
/// 标签索引 DocPath。
pub(crate) const TAGS: &str = "H:/Tags";

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
    pub(crate) document: Option<u128>,
}

/// 标签及其成员文档。
pub(crate) struct DocTag {
    pub(crate) id: u32,
    pub(crate) name: String,
    pub(crate) members: Vec<u128>,
}

/// 文档集：H: 卷文档、标签与内置页面。
pub(crate) struct DocSet {
    entries: Vec<DocEntry>,
    tags: Vec<DocTag>,
}

impl DocSet {
    /// 创建空文档集。
    pub(crate) fn new() -> Self {
        Self {
            entries: Vec::new(),
            tags: Vec::new(),
        }
    }

    /// 加入一页文档；DocPath 重复时返回 `false`。
    pub(crate) fn add(
        &mut self,
        id: String,
        title: String,
        body: String,
        document: Option<u128>,
    ) -> bool {
        if self.entries.iter().any(|entry| entry.id == id) {
            return false;
        }
        self.entries.push(DocEntry {
            id,
            title,
            body,
            document,
        });
        true
    }

    /// 补上没有卷时使用的内置页面。
    pub(crate) fn add_builtin(&mut self) {
        for (id, body) in BUILTIN {
            if self.contains(id) {
                continue;
            }
            let title = title_of(body).unwrap_or_else(|| doc_name(id));
            self.add(String::from(*id), title, String::from(*body), None);
        }
    }

    /// 设置标签视图。
    pub(crate) fn set_tags(&mut self, tags: Vec<DocTag>) {
        self.tags = tags;
    }

    /// 返回标签视图。
    pub(crate) fn tags(&self) -> &[DocTag] {
        &self.tags
    }

    /// 返回文档集里的全部页面。
    pub(crate) fn entries(&self) -> &[DocEntry] {
        &self.entries
    }

    /// 返回指定文档所属的标签名。
    pub(crate) fn tags_of(&self, document: u128) -> Vec<&str> {
        self.tags
            .iter()
            .filter(|tag| tag.members.contains(&document))
            .map(|tag| tag.name.as_str())
            .collect()
    }

    /// 判断文档集是否包含指定 DocPath。
    fn contains(&self, id: &str) -> bool {
        id == INDEX || id == TAGS || self.entries.iter().any(|entry| entry.id == id)
    }

    /// 按 DocPath 取页面；索引与标签页按当前集合即时生成。
    pub(crate) fn page(&self, id: &str) -> Option<DocEntry> {
        if id == INDEX {
            return Some(self.index());
        }
        if id == TAGS {
            return Some(self.tags_page());
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
        if !self.tags.is_empty() {
            body.push_str(&format!(
                "\n@heading(2, \"标签\")\n@link(\"{TAGS}\", \"全部标签（{}）\")\n",
                self.tags.len()
            ));
        }
        DocEntry {
            id: String::from(INDEX),
            title: String::from("文档索引"),
            body,
            document: None,
        }
    }

    /// 生成标签索引页面。
    fn tags_page(&self) -> DocEntry {
        let mut body = String::from("@title(\"标签\")\nH: 卷中的文档按标签分组。\n");
        if self.tags.is_empty() {
            body.push_str("\n暂无标签。\n");
        }
        for tag in &self.tags {
            body.push_str(&format!(
                "\n@heading(2, \"{}（{}）\")\n",
                tag.name,
                tag.members.len()
            ));
            if tag.members.is_empty() {
                body.push_str("@bullet(\"无成员\")\n");
                continue;
            }
            for document in &tag.members {
                match self.document_path(*document) {
                    Some(entry) => {
                        body.push_str(&format!("@link(\"{}\", \"{}\")\n", entry.id, entry.title));
                    }
                    None => body.push_str(&format!("@bullet(\"{document:032x}\")\n")),
                }
            }
        }
        DocEntry {
            id: String::from(TAGS),
            title: String::from("标签"),
            body,
            document: None,
        }
    }

    /// 按文档身份查找已加载的 DocPath。
    fn document_path(&self, document: u128) -> Option<&DocEntry> {
        self.entries
            .iter()
            .find(|entry| entry.document == Some(document))
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
#[derive(Clone)]
pub(crate) struct Run {
    pub(crate) text: String,
    pub(crate) style: Style,
    pub(crate) target: Option<String>,
}

/// 布局后的一行。
#[derive(Clone)]
pub(crate) struct Line {
    pub(crate) runs: Vec<Run>,
    pub(crate) code: bool,
}

/// 文档视图。
#[derive(Clone)]
pub(crate) struct DocView {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) lines: Vec<Line>,
    pub(crate) scroll: usize,
    pub(crate) history: Vec<String>,
    pub(crate) forward: Vec<String>,
    pub(crate) selected: Option<String>,
    pub(crate) font: &'static Font<'static>,
}

impl DocView {
    /// 从文档集加载一页并布局。
    pub(crate) fn load(
        set: &DocSet,
        id: &str,
        history: Vec<String>,
        forward: Vec<String>,
        font: &'static Font<'static>,
    ) -> Option<Self> {
        let entry = set.page(id)?;
        let document = entry.document;
        let mut view = Self {
            id: entry.id,
            title: entry.title,
            lines: Vec::new(),
            scroll: 0,
            history,
            forward,
            selected: None,
            font,
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
        if let Some(document) = document {
            let tags = set.tags_of(document);
            if !tags.is_empty() {
                view.lines.push(Line {
                    runs: Vec::new(),
                    code: false,
                });
                view.push_wrapped("标签", Style::Heading, false);
                for name in tags {
                    view.push_wrapped(&format!("· {name}"), Style::Bullet, false);
                }
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
                let width = char_width(self.font, character);
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
        (self.lines.len() * LINE_HEIGHT + MARGIN * 2).saturating_sub(CONTENT_HEIGHT)
    }

    /// 返回滚动百分比。
    pub(crate) fn scroll_percent(&self) -> usize {
        (self.scroll * 100)
            .checked_div(self.max_scroll())
            .map_or(100, |percent| percent.min(100))
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
        self.forward.clear();
        let history = core::mem::take(&mut self.history);
        let forward = core::mem::take(&mut self.forward);
        match Self::load(set, &entry.id, history, forward, self.font) {
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
        self.forward.push(self.id.clone());
        let history = core::mem::take(&mut self.history);
        let forward = core::mem::take(&mut self.forward);
        match Self::load(set, &previous, history, forward, self.font) {
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

    /// 前进到下一页。
    pub(crate) fn advance(&mut self, set: &DocSet) -> bool {
        let Some(next) = self.forward.pop() else {
            return false;
        };
        self.history.push(self.id.clone());
        let history = core::mem::take(&mut self.history);
        let forward = core::mem::take(&mut self.forward);
        match Self::load(set, &next, history, forward, self.font) {
            Some(view) => {
                *self = view;
                write(b"hk-server-holydoc: forward ");
                write(self.id.as_bytes());
                write(b"\n");
                true
            }
            None => false,
        }
    }

    /// 选中指定链接。
    pub(crate) fn select(&mut self, target: &str) {
        self.selected = Some(String::from(target));
    }

    /// 按文档顺序切换链接选择。
    pub(crate) fn select_link(&mut self, forward: bool) -> bool {
        let mut links: Vec<(usize, String)> = Vec::new();
        for (index, line) in self.lines.iter().enumerate() {
            for run in &line.runs {
                if let Some(target) = &run.target
                    && links.last().is_none_or(|(_, last)| last != target)
                {
                    links.push((index, target.clone()));
                }
            }
        }
        if links.is_empty() {
            self.selected = None;
            return false;
        }
        let current = self
            .selected
            .as_ref()
            .and_then(|target| links.iter().position(|(_, candidate)| candidate == target));
        let next = match (current, forward) {
            (Some(index), true) => (index + 1) % links.len(),
            (Some(index), false) => (index + links.len() - 1) % links.len(),
            (None, true) => 0,
            (None, false) => links.len() - 1,
        };
        let (line, target) = links[next].clone();
        self.selected = Some(target);
        self.ensure_visible(line);
        true
    }

    /// 打开当前选中的链接。
    pub(crate) fn open_selected(&mut self, set: &DocSet) -> bool {
        let Some(target) = self.selected.clone() else {
            return false;
        };
        self.open(set, &target)
    }

    /// 把指定行滚动到可视范围内。
    fn ensure_visible(&mut self, line: usize) {
        let top = line * LINE_HEIGHT;
        let bottom = top + LINE_HEIGHT;
        if self.scroll > top {
            self.scroll = top;
        }
        if MARGIN + bottom > self.scroll + CONTENT_HEIGHT {
            self.scroll = (MARGIN + bottom).saturating_sub(CONTENT_HEIGHT);
        }
        self.scroll = self.scroll.min(self.max_scroll());
    }

    pub(crate) fn link_at(&self, x: u32, y: u32) -> Option<String> {
        if y as usize >= CONTENT_HEIGHT {
            return None;
        }
        let adjusted = y as isize + self.scroll as isize - MARGIN as isize;
        if adjusted < 0 {
            return None;
        }
        let index = adjusted as usize / LINE_HEIGHT;
        let line = self.lines.get(index)?;
        let mut pen = MARGIN;
        for run in &line.runs {
            for character in run.text.chars() {
                let width = char_width(self.font, character);
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
            let mut pen = MARGIN;
            for run in &line.runs {
                for character in run.text.chars() {
                    if let Some(target) = &run.target {
                        return Some(((pen + 4) as u32, (y + 8) as u32, target.clone()));
                    }
                    pen += char_width(self.font, character);
                }
            }
            y += LINE_HEIGHT;
        }
        None
    }
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
