//! Workbench 模型：H: 文档树、编辑器缓冲、输出与命令。
//!
//! 文档经原生 Holy 协议读写；编辑器维护 UTF-8 字节游标，所有移动都落在字符
//! 边界。构建与运行是文档内的轻量操作：构建统计指令，运行求值 `@exec`。

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use hk_proto::holy::{DocId, HolyClient, KIND_COLLECTION, TOKEN_RIGHT_ADMIN, decode_entries};
use hk_server_api::{civil_time, kernel_stats, service_endpoint, ticks};

/// 编辑器缓冲上限。
pub(crate) const DOC_MAX: usize = 48 * 1024;
/// 输出保留行数。
const OUTPUT_LINES: usize = 64;
/// 集合递归深度上限。
const MAX_DEPTH: usize = 4;
/// 编辑器默认可见行数。
const DEFAULT_VISIBLE_LINES: usize = 20;
/// 编辑器默认可见字符列数。
const DEFAULT_VISIBLE_COLS: usize = 52;
/// 文档树默认可见行数。
const DEFAULT_NAV_ROWS: usize = 22;

/// 输入焦点。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Focus {
    /// 文档导航。
    Navigator,
    /// 编辑器。
    Editor,
}

/// 文档树中的一项。
pub(crate) struct DocEntry {
    /// DocPath。
    pub(crate) path: String,
    /// 显示名字。
    pub(crate) title: String,
    /// 文档身份。
    pub(crate) document: u128,
    /// 文档类型。
    pub(crate) kind: u8,
    /// 缩进层级。
    pub(crate) depth: usize,
}

/// 打开的文档。
pub(crate) struct Editor {
    /// DocPath。
    pub(crate) path: String,
    /// 标题。
    pub(crate) title: String,
    /// 文档身份。
    pub(crate) document: u128,
    /// 文本缓冲。
    pub(crate) text: String,
    /// 字节游标。
    pub(crate) cursor: usize,
    /// 顶部行。
    pub(crate) scroll_line: usize,
    /// 左侧字符列。
    pub(crate) scroll_col: usize,
    /// 是否有未保存修改。
    pub(crate) dirty: bool,
    /// 是否只读。
    pub(crate) readonly: bool,
    /// 可视行数。
    visible_lines: usize,
    /// 可视字符列数。
    visible_cols: usize,
}

/// 工作台状态。
pub(crate) struct Model {
    client: Box<HolyClient>,
    /// 文档树。
    pub(crate) entries: Vec<DocEntry>,
    /// 选中的树项。
    pub(crate) selected: usize,
    /// 树顶部行。
    pub(crate) nav_scroll: usize,
    /// 当前文档。
    pub(crate) editor: Option<Editor>,
    /// 输出面板。
    pub(crate) output: Vec<String>,
    /// 焦点。
    pub(crate) focus: Focus,
    /// 文档树可视行数。
    nav_rows: usize,
}

impl Model {
    /// 构造模型并加载 H: 文档树。
    pub(crate) fn new(endpoint: u64) -> Self {
        let mut model = Self {
            client: Box::new(HolyClient::new(endpoint)),
            entries: Vec::new(),
            selected: 0,
            nav_scroll: 0,
            editor: None,
            output: Vec::new(),
            focus: Focus::Navigator,
            nav_rows: DEFAULT_NAV_ROWS,
        };
        model.refresh();
        model
    }

    /// 重新加载文档树并保持当前选择。
    pub(crate) fn refresh(&mut self) -> usize {
        if service_endpoint("hk-server-holyfs") == 0 {
            self.log("H: 卷不可用：未找到 hk-server-holyfs");
            return 0;
        }
        let Ok(root) = self.client.root() else {
            self.log("H: 根集合不可用");
            return 0;
        };
        let mut entries = Vec::new();
        self.walk(root, "H:/", 0, &mut entries);
        let count = entries.len();
        self.entries = entries;
        if self.selected >= self.entries.len() {
            self.selected = self.entries.len().saturating_sub(1);
        }
        if self.editor.is_none() {
            if let Some(index) = self.path_index("H:/Welcome") {
                self.open(index);
            } else if let Some(index) = self.first_content() {
                self.open(index);
            }
        }
        self.log(&format!("文档树：{count} 项"));
        count
    }

    fn walk(&mut self, collection: DocId, path: &str, depth: usize, out: &mut Vec<DocEntry>) {
        if depth > MAX_DEPTH {
            return;
        }
        let result = self.client.enumerate_id(collection).map(|bytes| {
            decode_entries(bytes)
                .map(|entry| (entry.kind, entry.document, String::from(entry.name)))
                .collect::<Vec<_>>()
        });
        let entries = match result {
            Ok(entries) => entries,
            Err(status) => {
                self.log(&format!("枚举 {path} 失败：{status}"));
                return;
            }
        };
        for (kind, document, name) in entries {
            let child = join_path(path, &name);
            out.push(DocEntry {
                path: child.clone(),
                title: name,
                document,
                kind,
                depth,
            });
            if kind == KIND_COLLECTION {
                self.walk(DocId::new(document), &child, depth + 1, out);
            }
        }
    }

    /// 返回指定 DocPath 在树中的下标。
    pub(crate) fn path_index(&self, path: &str) -> Option<usize> {
        self.entries.iter().position(|entry| entry.path == path)
    }

    fn first_content(&self) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.kind != KIND_COLLECTION)
    }

    /// 选中并打开树项。
    pub(crate) fn open(&mut self, index: usize) -> bool {
        let Some(entry) = self.entries.get(index) else {
            return false;
        };
        let path = entry.path.clone();
        let title = entry.title.clone();
        let document = entry.document;
        let kind = entry.kind;
        let depth = entry.depth;
        self.selected = index;

        let (visible_lines, visible_cols) = self
            .editor
            .as_ref()
            .map_or((DEFAULT_VISIBLE_LINES, DEFAULT_VISIBLE_COLS), |editor| {
                (editor.visible_lines, editor.visible_cols)
            });
        let mut editor = Editor {
            path: path.clone(),
            title,
            document,
            text: String::new(),
            cursor: 0,
            scroll_line: 0,
            scroll_col: 0,
            dirty: false,
            readonly: kind == KIND_COLLECTION,
            visible_lines,
            visible_cols,
        };
        if kind == KIND_COLLECTION {
            editor.text = self.collection_listing(&path, depth);
        } else {
            let mut buffer = vec![0_u8; DOC_MAX];
            match self.client.read_id(DocId::new(document), 0, &mut buffer) {
                Ok(length) => {
                    let length = length.min(buffer.len());
                    editor.text = String::from_utf8_lossy(&buffer[..length]).into_owned();
                }
                Err(status) => {
                    editor.text = format!("# 读取失败：{status}\n");
                    editor.readonly = true;
                }
            }
        }
        self.editor = Some(editor);
        self.log(&format!("打开 {path}"));
        true
    }

    fn collection_listing(&self, path: &str, depth: usize) -> String {
        let mut body = format!("@title(\"{path}\")\n集合：{path}\n\n");
        let prefix = format!("{path}/");
        let mut found = false;
        for entry in &self.entries {
            if entry.depth == depth + 1 && entry.path.starts_with(&prefix) {
                found = true;
                if entry.kind == KIND_COLLECTION {
                    body.push_str(&format!("[集合] {}\n", entry.title));
                } else {
                    body.push_str(&format!("{}  {}\n", entry.title, entry.path));
                }
            }
        }
        if !found {
            body.push_str("集合为空。\n");
        }
        body
    }

    /// 选中树中的 DocPath。
    pub(crate) fn select_path(&mut self, path: &str) -> bool {
        let Some(index) = self.path_index(path) else {
            return false;
        };
        self.open(index)
    }

    /// 打开当前选中项。
    pub(crate) fn enter(&mut self) -> bool {
        self.open(self.selected)
    }

    /// 下一项。
    pub(crate) fn select_next(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        self.selected = (self.selected + 1).min(self.entries.len() - 1);
        self.ensure_selection_visible();
    }

    /// 上一项。
    pub(crate) fn select_previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
        self.ensure_selection_visible();
    }

    /// 滚动文档树。
    pub(crate) fn scroll_nav(&mut self, delta: isize) {
        if delta < 0 {
            self.selected = self.selected.saturating_sub(delta.unsigned_abs());
        } else {
            self.selected = (self.selected + delta as usize).min(self.entries.len());
            if self.selected == self.entries.len() {
                self.selected = self.entries.len().saturating_sub(1);
            }
        }
        self.ensure_selection_visible();
    }

    fn ensure_selection_visible(&mut self) {
        let rows = self.nav_rows.max(1);
        if self.selected < self.nav_scroll {
            self.nav_scroll = self.selected;
        } else if self.selected >= self.nav_scroll + rows {
            self.nav_scroll = self.selected + 1 - rows;
        }
    }

    /// 按窗口尺寸更新可见行列并保持游标可见。
    pub(crate) fn set_viewport(&mut self, nav_rows: usize, editor_rows: usize, editor_cols: usize) {
        self.nav_rows = nav_rows.max(1);
        if let Some(editor) = self.editor.as_mut() {
            editor.visible_lines = editor_rows.max(1);
            editor.visible_cols = editor_cols.max(1);
            editor.ensure_visible();
        }
        self.ensure_selection_visible();
    }

    /// 编辑器当前可视行数。
    pub(crate) fn visible_lines(&self) -> usize {
        self.editor
            .as_ref()
            .map_or(DEFAULT_VISIBLE_LINES, |editor| editor.visible_lines)
    }

    /// 文档树当前可视行数。
    pub(crate) fn nav_rows(&self) -> usize {
        self.nav_rows
    }

    /// 切换焦点。
    pub(crate) fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Navigator => Focus::Editor,
            Focus::Editor => Focus::Navigator,
        };
    }

    /// 插入字符。
    pub(crate) fn insert(&mut self, character: char) {
        let result = self.editor.as_mut().map(|editor| editor.insert(character));
        if let Some(Err(message)) = result {
            self.log(message);
        }
    }

    /// 删除前一个字符。
    pub(crate) fn backspace(&mut self) {
        let result = self.editor.as_mut().map(Editor::backspace);
        if let Some(Err(message)) = result {
            self.log(message);
        }
    }

    /// 删除后一个字符。
    pub(crate) fn delete(&mut self) {
        let result = self.editor.as_mut().map(Editor::delete);
        if let Some(Err(message)) = result {
            self.log(message);
        }
    }

    /// 处理编辑器移动键。
    pub(crate) fn move_editor(&mut self, key: EditorKey) {
        if let Some(editor) = self.editor.as_mut() {
            editor.move_cursor(key);
        }
    }

    /// 把游标放到指定行与字符列。
    pub(crate) fn set_cursor(&mut self, line: usize, column: usize) {
        if let Some(editor) = self.editor.as_mut() {
            editor.cursor = offset_of(&editor.text, line, column);
            editor.ensure_visible();
        }
    }

    /// 编辑器滚轮。
    pub(crate) fn scroll_editor(&mut self, delta: isize) {
        if let Some(editor) = self.editor.as_mut() {
            let lines = editor.line_count();
            let max = lines.saturating_sub(editor.visible_lines);
            if delta < 0 {
                editor.scroll_line = editor.scroll_line.saturating_sub(delta.unsigned_abs());
            } else {
                editor.scroll_line = (editor.scroll_line + delta as usize).min(max);
            }
        }
    }

    /// 保存当前文档。
    pub(crate) fn save(&mut self) {
        let Some(editor) = self.editor.as_ref() else {
            return;
        };
        if editor.readonly {
            self.log("只读文档不能保存");
            return;
        }
        let path = editor.path.clone();
        let document = DocId::new(editor.document);
        let bytes = editor.text.clone();
        let client = &mut self.client;
        let result = (|| -> Result<(), u8> {
            client.write_id(document, 0, bytes.as_bytes())?;
            client.truncate_id(document, bytes.len() as u64)?;
            client.sync()
        })();
        match result {
            Ok(()) => {
                if let Some(editor) = self.editor.as_mut() {
                    editor.dirty = false;
                }
                self.log(&format!("保存 {path}（{} 字节）", bytes.len()));
            }
            Err(status) => self.log(&format!("保存 {path} 失败：{status}")),
        }
    }

    /// 构建当前文档，返回摘要。
    pub(crate) fn build(&mut self) -> String {
        let Some(editor) = self.editor.as_ref() else {
            return String::from("build 失败：没有打开的文档");
        };
        let lines = editor.text.lines().count();
        let directives = editor
            .text
            .lines()
            .filter(|line| line.trim_start().starts_with('@'))
            .count();
        let summary = format!("build ok：{lines} 行，{directives} 条指令");
        self.log(&summary);
        summary
    }

    /// 运行文档中的 `@exec`，返回执行的命令数。
    pub(crate) fn run(&mut self) -> usize {
        let Some(editor) = self.editor.as_ref() else {
            self.log("run 失败：没有打开的文档");
            return 0;
        };
        let commands: Vec<String> = editor
            .text
            .lines()
            .filter_map(|line| line.trim().strip_prefix("@exec"))
            .map(|command| command.trim().to_string())
            .collect();
        let count = commands.len();
        self.log(&format!("run：{count} 个 @exec"));
        for command in commands {
            let result = evaluate(&command);
            self.log(&format!("  {command} → {result}"));
        }
        count
    }

    /// 读取最近一次用户异常回溯。
    pub(crate) fn debug(&mut self) -> usize {
        let text = hk_debug::fault_text();
        let mut lines = 0;
        for line in text.lines().take(12) {
            self.log(&format!("debug | {line}"));
            lines += 1;
        }
        if lines == 0 {
            self.log("debug | 没有异常记录");
        }
        lines
    }

    /// 创建临时文档。
    pub(crate) fn create_document(&mut self, name: &str) -> Result<u128, u8> {
        let root = self.client.root()?;
        self.client
            .create(root, name, hk_proto::holy::DocumentKind::Content)
            .map(DocId::get)
    }

    /// 读取文档内容。
    pub(crate) fn read_document(&mut self, document: u128) -> Result<String, u8> {
        let mut buffer = vec![0_u8; DOC_MAX];
        let length = self.client.read_id(DocId::new(document), 0, &mut buffer)?;
        let length = length.min(buffer.len());
        Ok(String::from_utf8_lossy(&buffer[..length]).into_owned())
    }

    /// 销毁临时文档。
    pub(crate) fn purge_document(&mut self, document: u128) -> Result<(), u8> {
        let admin = self.client.acquire_volume(TOKEN_RIGHT_ADMIN)?;
        let result = self.client.purge(admin, DocId::new(document));
        let _ = self.client.release_volume(admin);
        result
    }

    /// 把编辑器切到指定文档，用于自检与新建文档。
    #[cfg_attr(not(target_os = "none"), allow(dead_code))]
    pub(crate) fn load_editor(&mut self, document: u128, path: &str, title: &str, text: &str) {
        let (visible_lines, visible_cols) = self
            .editor
            .as_ref()
            .map_or((DEFAULT_VISIBLE_LINES, DEFAULT_VISIBLE_COLS), |editor| {
                (editor.visible_lines, editor.visible_cols)
            });
        self.editor = Some(Editor {
            path: String::from(path),
            title: String::from(title),
            document,
            text: String::from(text),
            cursor: text.len(),
            scroll_line: 0,
            scroll_col: 0,
            dirty: false,
            readonly: false,
            visible_lines,
            visible_cols,
        });
    }

    /// 追加一行输出。
    pub(crate) fn log(&mut self, line: &str) {
        for entry in line.lines() {
            self.output.push(String::from(entry));
        }
        if self.output.len() > OUTPUT_LINES {
            let excess = self.output.len() - OUTPUT_LINES;
            self.output.drain(..excess);
        }
    }
}

/// 编辑器移动键。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditorKey {
    /// 左。
    Left,
    /// 右。
    Right,
    /// 上。
    Up,
    /// 下。
    Down,
    /// 行首。
    Home,
    /// 行尾。
    End,
}

impl Editor {
    fn insert(&mut self, character: char) -> Result<(), &'static str> {
        if self.readonly {
            return Err("只读文档");
        }
        if self.text.len() + character.len_utf8() > DOC_MAX {
            return Err("超过文档缓冲上限");
        }
        self.cursor = self.cursor.min(self.text.len());
        self.text.insert(self.cursor, character);
        self.cursor += character.len_utf8();
        self.dirty = true;
        self.ensure_visible();
        Ok(())
    }

    fn backspace(&mut self) -> Result<(), &'static str> {
        if self.readonly {
            return Err("只读文档");
        }
        if self.cursor == 0 {
            return Ok(());
        }
        let mut start = self.cursor - 1;
        while start > 0 && !self.text.is_char_boundary(start) {
            start -= 1;
        }
        self.text.remove(start);
        self.cursor = start;
        self.dirty = true;
        self.ensure_visible();
        Ok(())
    }

    fn delete(&mut self) -> Result<(), &'static str> {
        if self.readonly {
            return Err("只读文档");
        }
        if self.cursor >= self.text.len() {
            return Ok(());
        }
        self.text.remove(self.cursor);
        self.dirty = true;
        self.ensure_visible();
        Ok(())
    }

    fn move_cursor(&mut self, key: EditorKey) {
        let (line, column) = line_column(&self.text, self.cursor);
        match key {
            EditorKey::Left => {
                if self.cursor > 0 {
                    let mut start = self.cursor - 1;
                    while start > 0 && !self.text.is_char_boundary(start) {
                        start -= 1;
                    }
                    self.cursor = start;
                }
            }
            EditorKey::Right => {
                if self.cursor < self.text.len() {
                    let mut end = self.cursor + 1;
                    while end < self.text.len() && !self.text.is_char_boundary(end) {
                        end += 1;
                    }
                    self.cursor = end;
                }
            }
            EditorKey::Up => {
                if line > 0 {
                    self.cursor = offset_of(&self.text, line - 1, column);
                }
            }
            EditorKey::Down => {
                let last = self.line_count().saturating_sub(1);
                if line < last {
                    self.cursor = offset_of(&self.text, line + 1, column);
                }
            }
            EditorKey::Home => {
                self.cursor = offset_of(&self.text, line, 0);
            }
            EditorKey::End => {
                self.cursor = offset_of(&self.text, line, usize::MAX);
            }
        }
        self.ensure_visible();
    }

    fn line_count(&self) -> usize {
        self.text.split('\n').count().max(1)
    }

    fn ensure_visible(&mut self) {
        let (line, column) = line_column(&self.text, self.cursor);
        if line < self.scroll_line {
            self.scroll_line = line;
        } else if line >= self.scroll_line + self.visible_lines {
            self.scroll_line = line + 1 - self.visible_lines;
        }
        if column < self.scroll_col {
            self.scroll_col = column;
        } else if column >= self.scroll_col + self.visible_cols {
            self.scroll_col = column + 1 - self.visible_cols;
        }
    }
}

/// 计算字节游标所在的行与字符列。
pub(crate) fn line_column(text: &str, cursor: usize) -> (usize, usize) {
    let mut line = 0;
    let mut column = 0;
    for (index, character) in text.char_indices() {
        if index >= cursor {
            break;
        }
        if character == '\n' {
            line += 1;
            column = 0;
        } else {
            column += 1;
        }
    }
    (line, column)
}

/// 计算指定行与字符列的字节偏移。
pub(crate) fn offset_of(text: &str, line: usize, column: usize) -> usize {
    let mut current_line = 0;
    let mut current_column = 0;
    for (index, character) in text.char_indices() {
        if current_line == line {
            if current_column == column || character == '\n' {
                return index;
            }
            current_column += 1;
        } else if character == '\n' {
            current_line += 1;
            if current_line == line {
                current_column = 0;
            }
        }
    }
    text.len()
}

/// 返回指定行的文本。
pub(crate) fn line_text(text: &str, line: usize) -> &str {
    text.split('\n').nth(line).unwrap_or("")
}

fn join_path(parent: &str, name: &str) -> String {
    if parent.ends_with('/') {
        format!("{parent}{name}")
    } else {
        format!("{parent}/{name}")
    }
}

fn evaluate(command: &str) -> String {
    let mut parts = command.split_whitespace();
    match parts.next() {
        Some("version") => String::from("Workbench 0.1 · HolyKernel M6"),
        Some("time") => match civil_time() {
            Some(time) => format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                time.year, time.month, time.day, time.hour, time.minute, time.second
            ),
            None => String::from("民用时间不可用"),
        },
        Some("ticks") => format!("tick {}", ticks()),
        Some("echo") => parts.collect::<Vec<_>>().join(" "),
        Some("stats") => {
            let stats = kernel_stats();
            format!(
                "threads {} objects {} runnable {}",
                stats.threads, stats.objects, stats.runnable
            )
        }
        Some("documents") | Some("docs") => String::from("见左侧文档树"),
        Some(_) => String::from("未知命令"),
        None => String::from("空命令"),
    }
}

#[cfg(test)]
mod tests {
    use super::{line_column, offset_of};

    #[test]
    fn maps_cursor_to_line_and_column() {
        let text = "ab\ncde\n";
        assert_eq!(line_column(text, 0), (0, 0));
        assert_eq!(line_column(text, 2), (0, 2));
        assert_eq!(line_column(text, 3), (1, 0));
        assert_eq!(line_column(text, 6), (1, 3));
        assert_eq!(line_column(text, 7), (2, 0));
    }

    #[test]
    fn maps_line_and_column_to_offset() {
        let text = "ab\ncde\n";
        assert_eq!(offset_of(text, 0, 0), 0);
        assert_eq!(offset_of(text, 0, 2), 2);
        assert_eq!(offset_of(text, 1, 0), 3);
        assert_eq!(offset_of(text, 1, 2), 5);
        assert_eq!(offset_of(text, 2, 0), 7);
        assert_eq!(offset_of(text, 1, usize::MAX), 6);
    }
}
