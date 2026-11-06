//! Workbench 绘制：标题栏、文档树、编辑器与输出面板。

use alloc::format;
use alloc::string::String;

use hk_canvas::{Rect, Surface, text_width};
use hk_font::Font;
use hk_proto::holy::KIND_COLLECTION;

use crate::model::{Focus, Model, line_column};

/// 初始窗口宽度。
pub(crate) const WIDTH: usize = 760;
/// 初始窗口高度。
pub(crate) const HEIGHT: usize = 520;

const HEADER: usize = 28;
const SIDEBAR: usize = 220;
const FOOTER: usize = 22;
const OUTPUT: usize = 104;
const ROW: usize = 18;
const PAD: usize = 8;
const LINE_NO: usize = 34;

const BG: u32 = 0x0014_1A20;
const PANEL: u32 = 0x0010_1820;
const HEADER_BG: u32 = 0x001C_2632;
const BORDER: u32 = 0x002C_3A46;
const TEXT: u32 = 0x00C8_E0D0;
const MUTED: u32 = 0x0070_8898;
const ACCENT: u32 = 0x0080_C0FF;
const SELECT: u32 = 0x002A_4258;
const CURRENT_LINE: u32 = 0x0018_242E;
const CURSOR: u32 = 0x00A0_D0FF;
const OUTPUT_BG: u32 = 0x0010_1418;
const DIR: u32 = 0x00E0_C080;
const WARN: u32 = 0x00E0_9080;
const GUTTER: u32 = 0x0050_6878;

/// 随窗口尺寸变化的布局。
#[derive(Clone, Copy, Debug)]
pub(crate) struct Layout {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) sidebar: usize,
    pub(crate) output_top: usize,
    pub(crate) body_top: usize,
    pub(crate) nav_top: usize,
    pub(crate) text_left: usize,
    pub(crate) text_right: usize,
    pub(crate) row: usize,
    pub(crate) editor_rows: usize,
    pub(crate) nav_rows: usize,
    pub(crate) editor_cols: usize,
}

impl Layout {
    /// 按窗口尺寸计算栏宽、面板高度与可视行数。
    pub(crate) fn new(width: usize, height: usize) -> Self {
        let sidebar = SIDEBAR.min(width.saturating_sub(120)).max(80);
        let output = OUTPUT.min(height / 3).max(40);
        let output_top = height.saturating_sub(FOOTER + output);
        let body_top = HEADER + 4;
        let nav_top = HEADER + 28;
        let text_left = sidebar + PAD + LINE_NO;
        let text_right = width.saturating_sub(PAD);
        Self {
            width,
            height,
            sidebar,
            output_top,
            body_top,
            nav_top,
            text_left,
            text_right,
            row: ROW,
            editor_rows: output_top.saturating_sub(body_top) / ROW,
            nav_rows: height.saturating_sub(FOOTER + nav_top) / ROW,
            editor_cols: text_right.saturating_sub(text_left) / 8,
        }
    }
}

/// 绘制整个工作台。
pub(crate) fn render(model: &Model, font: &Font, surface: &Surface) {
    let layout = Layout::new(surface.width(), surface.height());
    surface.fill(
        Rect::new(0, 0, layout.width as i32, layout.height as i32),
        BG,
    );
    draw_header(model, font, surface, &layout);
    draw_navigator(model, font, surface, &layout);
    draw_editor(model, font, surface, &layout);
    draw_output(model, font, surface, &layout);
    draw_footer(model, font, surface, &layout);
    surface.fill(
        Rect::new(
            layout.sidebar as i32 - 1,
            HEADER as i32,
            1,
            layout.height.saturating_sub(HEADER) as i32,
        ),
        BORDER,
    );
    surface.fill(
        Rect::new(
            layout.sidebar as i32,
            layout.output_top as i32 - 1,
            layout.width.saturating_sub(layout.sidebar) as i32,
            1,
        ),
        BORDER,
    );
    surface.fill(
        Rect::new(0, HEADER as i32 - 1, layout.width as i32, 1),
        BORDER,
    );
}

fn draw_header(model: &Model, font: &Font, surface: &Surface, layout: &Layout) {
    surface.fill(
        Rect::new(0, 0, layout.width as i32, HEADER as i32),
        HEADER_BG,
    );
    let mut x = PAD as i32;
    surface.draw_text(&mut x, 6, "Workbench", ACCENT, font);
    x += 10;
    surface.draw_text(&mut x, 6, "·", MUTED, font);
    x += 10;
    let focus = match model.focus {
        Focus::Navigator => "文档树",
        Focus::Editor => "编辑器",
    };
    surface.draw_text(&mut x, 6, focus, TEXT, font);

    let title = match &model.editor {
        Some(editor) => {
            if editor.dirty {
                format!("{} *", editor.path)
            } else {
                editor.path.clone()
            }
        }
        None => String::from("没有打开的文档"),
    };
    let mut x = layout.width as i32 - PAD as i32 - text_width(font, &title);
    draw_clipped(
        surface,
        font,
        &mut x,
        6,
        layout.width as i32 - PAD as i32,
        MUTED,
        &title,
    );
}

fn draw_navigator(model: &Model, font: &Font, surface: &Surface, layout: &Layout) {
    surface.fill(
        Rect::new(
            0,
            HEADER as i32,
            layout.sidebar as i32,
            layout.height.saturating_sub(HEADER) as i32,
        ),
        PANEL,
    );
    let mut x = PAD as i32;
    surface.draw_text(&mut x, (HEADER + 8) as i32, "文档", MUTED, font);
    let count = format!("{}", model.entries.len());
    let mut x = layout.sidebar as i32 - PAD as i32 - text_width(font, &count);
    surface.draw_text(&mut x, (HEADER + 8) as i32, &count, MUTED, font);

    if model.entries.is_empty() {
        let mut x = PAD as i32;
        surface.draw_text(&mut x, layout.nav_top as i32, "没有 H: 文档", WARN, font);
        return;
    }
    for row in 0..layout.nav_rows {
        let index = model.nav_scroll + row;
        let Some(entry) = model.entries.get(index) else {
            break;
        };
        let y = layout.nav_top + row * ROW;
        if index == model.selected {
            surface.fill(
                Rect::new(0, y as i32, layout.sidebar as i32, ROW as i32),
                SELECT,
            );
        }
        let indent = PAD + entry.depth * 10;
        let is_collection = entry.kind == KIND_COLLECTION;
        let label = if is_collection {
            format!("{}/", entry.title)
        } else {
            entry.title.clone()
        };
        let color = if is_collection { DIR } else { TEXT };
        let mut x = indent as i32;
        draw_clipped(
            surface,
            font,
            &mut x,
            y as i32,
            layout.sidebar as i32 - PAD as i32,
            color,
            &label,
        );
    }
}

fn draw_editor(model: &Model, font: &Font, surface: &Surface, layout: &Layout) {
    surface.fill(
        Rect::new(
            layout.sidebar as i32,
            HEADER as i32,
            layout.width.saturating_sub(layout.sidebar) as i32,
            layout.output_top.saturating_sub(HEADER) as i32,
        ),
        BG,
    );
    let Some(editor) = &model.editor else {
        let mut x = layout.text_left as i32;
        surface.draw_text(
            &mut x,
            layout.body_top as i32 + 8,
            "从左侧选择文档",
            MUTED,
            font,
        );
        return;
    };
    let lines: alloc::vec::Vec<&str> = editor.text.split('\n').collect();
    let (cursor_line, cursor_column) = line_column(&editor.text, editor.cursor);
    for row in 0..layout.editor_rows {
        let line_index = editor.scroll_line + row;
        let Some(line) = lines.get(line_index) else {
            break;
        };
        let y = layout.body_top + row * ROW;
        if line_index == cursor_line {
            surface.fill(
                Rect::new(
                    layout.sidebar as i32,
                    y as i32,
                    layout.width.saturating_sub(layout.sidebar) as i32,
                    ROW as i32,
                ),
                CURRENT_LINE,
            );
        }
        let number = format!("{}", line_index + 1);
        let mut x =
            layout.sidebar as i32 + PAD as i32 + LINE_NO as i32 - 4 - text_width(font, &number);
        surface.draw_text(&mut x, y as i32 + 1, &number, GUTTER, font);

        let color = line_color(line);
        let mut pen = layout.text_left as i32;
        let mut column = 0;
        for character in line.chars() {
            if column < editor.scroll_col {
                column += 1;
                continue;
            }
            if pen >= layout.text_right as i32 {
                break;
            }
            let cursor_here = line_index == cursor_line && column == cursor_column;
            if cursor_here {
                let advance = font.advance(character).max(2);
                surface.fill(Rect::new(pen, y as i32, advance, ROW as i32), CURSOR);
                surface.draw_glyph(pen, y as i32 + 1, BG, character, font);
            } else {
                surface.draw_glyph(pen, y as i32 + 1, color, character, font);
            }
            pen += font.advance(character);
            column += 1;
        }
        if line_index == cursor_line
            && cursor_column >= editor.scroll_col
            && cursor_column >= column
            && pen < layout.text_right as i32
        {
            surface.fill(Rect::new(pen, y as i32, 2, ROW as i32), CURSOR);
        }
    }

    if editor.text.is_empty() && editor.scroll_line == 0 {
        let mut x = layout.text_left as i32;
        surface.draw_text(
            &mut x,
            layout.body_top as i32 + 1,
            "（空文档）",
            MUTED,
            font,
        );
    }
}

fn draw_output(model: &Model, font: &Font, surface: &Surface, layout: &Layout) {
    let output_height = layout
        .height
        .saturating_sub(FOOTER)
        .saturating_sub(layout.output_top);
    surface.fill(
        Rect::new(
            layout.sidebar as i32,
            layout.output_top as i32,
            layout.width.saturating_sub(layout.sidebar) as i32,
            output_height as i32,
        ),
        OUTPUT_BG,
    );
    let mut x = layout.sidebar as i32 + PAD as i32;
    surface.draw_text(&mut x, layout.output_top as i32 + 4, "输出", MUTED, font);

    let rows = output_height.saturating_sub(24) / 16;
    let start = model.output.len().saturating_sub(rows);
    for (row, line) in model.output[start..].iter().enumerate() {
        let y = layout.output_top + 22 + row * 16;
        let color = if line.contains("失败") || line.contains("不可用") {
            WARN
        } else if line.starts_with("  ") {
            MUTED
        } else {
            TEXT
        };
        let mut x = layout.sidebar as i32 + PAD as i32;
        draw_clipped(
            surface,
            font,
            &mut x,
            y as i32,
            layout.width as i32 - PAD as i32,
            color,
            line,
        );
    }
}

fn draw_footer(model: &Model, font: &Font, surface: &Surface, layout: &Layout) {
    surface.fill(
        Rect::new(
            0,
            layout.height.saturating_sub(FOOTER) as i32,
            layout.width as i32,
            FOOTER as i32,
        ),
        HEADER_BG,
    );
    let hint = match model.focus {
        Focus::Navigator => "Tab 编辑器 · ↑/↓ 选择 · Enter 打开 · Ctrl+R 刷新 · Ctrl+Q 退出",
        Focus::Editor => {
            "Tab 文档树 · Ctrl+S 保存 · Ctrl+B 构建 · Ctrl+R 运行 · Ctrl+D 调试 · Ctrl+Q 退出"
        }
    };
    let mut x = PAD as i32;
    draw_clipped(
        surface,
        font,
        &mut x,
        layout.height.saturating_sub(FOOTER) as i32 + 3,
        layout.width as i32 - PAD as i32,
        MUTED,
        hint,
    );
}

fn line_color(line: &str) -> u32 {
    let trimmed = line.trim_start();
    if trimmed.starts_with('@') {
        ACCENT
    } else if trimmed.starts_with('#') {
        GUTTER
    } else {
        TEXT
    }
}

/// 绘制文本，起点越过 `right` 时停止。
fn draw_clipped(
    surface: &Surface,
    font: &Font,
    pen: &mut i32,
    y: i32,
    right: i32,
    color: u32,
    text: &str,
) {
    for character in text.chars() {
        if *pen >= right {
            break;
        }
        surface.draw_glyph(*pen, y, color, character, font);
        *pen = (*pen).saturating_add(font.advance(character));
    }
}
