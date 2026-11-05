//! 文档模型：Markdown 子集解析、自动换行布局、链接与历史。

use alloc::format;
use alloc::string::String;
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

/// 文档来源。
pub(crate) struct DocSource {
    pub(crate) id: &'static str,
    pub(crate) title: &'static str,
    pub(crate) body: &'static str,
}

/// 内置文档集。
pub(crate) const DOCS: &[DocSource] = &[
    DocSource {
        id: "index",
        title: "首页",
        body: r#"# HolyDoc
活文档窗口 · HolyKernel M6

这是一个可滚动的原生文档窗口。文档使用 Markdown 子集解析、自动换行布局，并渲染到共享表面。

## 导航
- [架构总览](architecture)
- [Crate 分层](crates)
- [路线图](roadmap)

## 执行
@exec version
@exec ticks

## 操作
- j / k 滚动
- 空格 / b 翻页
- 点击链接跳转
- Backspace 返回上一页
- q 或标题栏关闭按钮退出
"#,
    },
    DocSource {
        id: "architecture",
        title: "架构总览",
        body: r#"# 架构总览
HolyKernel 是分层微内核：内核只提供对象、地址空间与 IPC，驱动、文件系统与网络都在用户态。

## 层次
- L0 基础类型与内核对象
- L1 内核库
- L2 用户态库
- L3 常驻服务
- L4 平台与装配

## 服务协作
服务之间只通过 Endpoint、Handle 与线协议通信，不链接彼此的实现。

```text
Console -> Display -> Framebuffer
HolyDoc -> Display -> Framebuffer
VFS -> virtio -> block
```

[返回首页](index)
"#,
    },
    DocSource {
        id: "crates",
        title: "Crate 分层",
        body: r#"# Crate 分层
## 基础
hk-base 提供地址、架构与启动信息；hk-hal 在其上定义平台 trait。

## 内核
hk-kernel、hk-handle、hk-ipc、hk-mem 构成对象与 IPC 核心。

## 用户态
hk-server-api 提供系统调用薄封装与服务协议；hk-runtime 承载用户任务视图。

```text
hk-base -> hk-hal -> hk-kernel
hk-server-api -> servers/*
```

[架构总览](architecture) · [返回首页](index)
"#,
    },
    DocSource {
        id: "roadmap",
        title: "路线图",
        body: r#"# 路线图
## 已完成
- M0-M5：启动、内核对象、驱动、VFS、网络
- M6：Display、输入、Console、窗口管理与 HolyDoc

## 进行中
- 原生欢迎页与文档浏览
- 主题与字体缩放
- 调试与热重启

## 后续
@exec docs

[返回首页](index)
"#,
    },
];

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
    pub(crate) target: Option<&'static str>,
}

/// 布局后的一行。
pub(crate) struct Line {
    pub(crate) runs: Vec<Run>,
    pub(crate) code: bool,
}

/// 文档视图。
pub(crate) struct DocView {
    pub(crate) id: &'static str,
    pub(crate) title: &'static str,
    pub(crate) lines: Vec<Line>,
    pub(crate) scroll: usize,
    pub(crate) history: Vec<&'static str>,
}

impl DocView {
    /// 从文档源加载并布局。
    pub(crate) fn load(doc: &'static DocSource, history: Vec<&'static str>) -> Self {
        let mut view = Self {
            id: doc.id,
            title: doc.title,
            lines: Vec::new(),
            scroll: 0,
            history,
        };
        let mut code = false;
        for raw in doc.body.lines() {
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
                view.push_inline(rest, Style::Title, false);
            } else if let Some(rest) = line.strip_prefix("## ") {
                view.push_inline(rest, Style::Heading, false);
            } else if let Some(rest) = line.strip_prefix("- ") {
                view.push_inline(&format!("· {rest}"), Style::Bullet, false);
            } else if let Some(rest) = line.strip_prefix("@exec ") {
                for output in exec(rest) {
                    view.push_wrapped(&output, Style::Code, true);
                }
            } else {
                view.push_inline(line, Style::Body, false);
            }
        }
        if view.lines.is_empty() {
            view.lines.push(Line {
                runs: Vec::new(),
                code: false,
            });
        }
        view
    }

    fn push_inline(&mut self, text: &str, style: Style, code: bool) {
        self.wrap(parse_inline(text, style), code);
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
                append_run(&mut current, character, run.style, run.target);
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

    pub(crate) fn open(&mut self, target: &'static str) -> bool {
        if target == self.id {
            return false;
        }
        let Some(doc) = DOCS.iter().find(|doc| doc.id == target) else {
            return false;
        };
        self.history.push(self.id);
        let history = core::mem::take(&mut self.history);
        *self = Self::load(doc, history);
        write(b"hk-server-holydoc: open ");
        write(target.as_bytes());
        write(b"\n");
        true
    }

    pub(crate) fn back(&mut self) -> bool {
        let Some(previous) = self.history.pop() else {
            return false;
        };
        let Some(doc) = DOCS.iter().find(|doc| doc.id == previous) else {
            return false;
        };
        let history = core::mem::take(&mut self.history);
        *self = Self::load(doc, history);
        write(b"hk-server-holydoc: back ");
        write(self.id.as_bytes());
        write(b"\n");
        true
    }

    pub(crate) fn link_at(&self, x: u32, y: u32) -> Option<&'static str> {
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
                if let Some(target) = run.target
                    && x as usize >= pen
                    && (x as usize) < pen + width
                {
                    return Some(target);
                }
                pen += width;
            }
        }
        None
    }

    pub(crate) fn first_link(&self) -> Option<(u32, u32, &'static str)> {
        let mut y = MARGIN;
        for line in &self.lines {
            let mut pen = 0;
            for run in &line.runs {
                for character in run.text.chars() {
                    if let Some(target) = run.target {
                        return Some(((pen + 4) as u32, (y + 8) as u32, target));
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

/// 向当前行追加一个字符，合并同样式片段。
fn append_run(runs: &mut Vec<Run>, character: char, style: Style, target: Option<&'static str>) {
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
fn parse_inline(text: &str, style: Style) -> Vec<Run> {
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
        let target = doc_id(&rest[close + 2..end]);
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

/// 把链接目标解析为内置文档编号。
fn doc_id(name: &str) -> Option<&'static str> {
    DOCS.iter().find(|doc| doc.id == name).map(|doc| doc.id)
}
