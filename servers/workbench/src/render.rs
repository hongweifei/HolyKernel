//! Workbench 绘制：标题栏、文档树、编辑器与输出面板。

use alloc::format;
use alloc::string::String;

use hk_canvas::{Rect, Surface, text_width};
use hk_font::Font;
use hk_server_api::holy::KIND_COLLECTION;

use crate::model::{Focus, Model, VISIBLE_LINES, line_column};

/// 窗口宽度。
pub(crate) const WIDTH: usize = 760;
/// 窗口高度。
pub(crate) const HEIGHT: usize = 520;

const HEADER: usize = 28;
pub(crate) const SIDEBAR: usize = 220;
const FOOTER: usize = 22;
const OUTPUT: usize = 104;
pub(crate) const ROW: usize = 18;
const PAD: usize = 8;
const LINE_NO: usize = 34;

pub(crate) const NAV_TOP: usize = HEADER + 28;
pub(crate) const BODY_TOP: usize = HEADER + 4;
pub(crate) const OUTPUT_TOP: usize = HEIGHT - FOOTER - OUTPUT;
pub(crate) const TEXT_LEFT: usize = SIDEBAR + PAD + LINE_NO;
pub(crate) const TEXT_RIGHT: usize = WIDTH - PAD;

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

/// 绘制整个工作台。
pub(crate) fn render(model: &Model, font: &Font, surface: &Surface) {
    surface.fill(Rect::new(0, 0, WIDTH as i32, HEIGHT as i32), BG);
    draw_header(model, font, surface);
    draw_navigator(model, font, surface);
    draw_editor(model, font, surface);
    draw_output(model, font, surface);
    draw_footer(model, font, surface);
    surface.fill(
        Rect::new(
            SIDEBAR as i32 - 1,
            HEADER as i32,
            1,
            (HEIGHT - HEADER) as i32,
        ),
        BORDER,
    );
    surface.fill(
        Rect::new(
            SIDEBAR as i32,
            OUTPUT_TOP as i32 - 1,
            (WIDTH - SIDEBAR) as i32,
            1,
        ),
        BORDER,
    );
    surface.fill(Rect::new(0, HEADER as i32 - 1, WIDTH as i32, 1), BORDER);
}

fn draw_header(model: &Model, font: &Font, surface: &Surface) {
    surface.fill(Rect::new(0, 0, WIDTH as i32, HEADER as i32), HEADER_BG);
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
    let mut x = WIDTH as i32 - PAD as i32 - text_width(font, &title);
    surface.draw_text(&mut x, 6, &title, MUTED, font);
}

fn draw_navigator(model: &Model, font: &Font, surface: &Surface) {
    surface.fill(
        Rect::new(0, HEADER as i32, SIDEBAR as i32, (HEIGHT - HEADER) as i32),
        PANEL,
    );
    let mut x = PAD as i32;
    surface.draw_text(&mut x, (HEADER + 8) as i32, "文档", MUTED, font);
    let count = format!("{}", model.entries.len());
    let mut x = SIDEBAR as i32 - PAD as i32 - text_width(font, &count);
    surface.draw_text(&mut x, (HEADER + 8) as i32, &count, MUTED, font);

    let list_top = HEADER + 28;
    let rows = (HEIGHT - FOOTER - list_top) / ROW;
    if model.entries.is_empty() {
        let mut x = PAD as i32;
        surface.draw_text(&mut x, list_top as i32, "没有 H: 文档", WARN, font);
        return;
    }
    for row in 0..rows {
        let index = model.nav_scroll + row;
        let Some(entry) = model.entries.get(index) else {
            break;
        };
        let y = list_top + row * ROW;
        if index == model.selected {
            surface.fill(Rect::new(0, y as i32, SIDEBAR as i32, ROW as i32), SELECT);
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
            (SIDEBAR - PAD) as i32,
            color,
            &label,
        );
    }
}

fn draw_editor(model: &Model, font: &Font, surface: &Surface) {
    surface.fill(
        Rect::new(
            SIDEBAR as i32,
            HEADER as i32,
            (WIDTH - SIDEBAR) as i32,
            (OUTPUT_TOP - HEADER) as i32,
        ),
        BG,
    );
    let Some(editor) = &model.editor else {
        let mut x = TEXT_LEFT as i32;
        surface.draw_text(&mut x, BODY_TOP as i32 + 8, "从左侧选择文档", MUTED, font);
        return;
    };
    let lines: alloc::vec::Vec<&str> = editor.text.split('\n').collect();
    let (cursor_line, cursor_column) = line_column(&editor.text, editor.cursor);
    for row in 0..VISIBLE_LINES {
        let line_index = editor.scroll_line + row;
        let Some(line) = lines.get(line_index) else {
            break;
        };
        let y = BODY_TOP + row * ROW;
        if line_index == cursor_line {
            surface.fill(
                Rect::new(
                    SIDEBAR as i32,
                    y as i32,
                    (WIDTH - SIDEBAR) as i32,
                    ROW as i32,
                ),
                CURRENT_LINE,
            );
        }
        let number = format!("{}", line_index + 1);
        let mut x = SIDEBAR as i32 + PAD as i32 + LINE_NO as i32 - 4 - text_width(font, &number);
        surface.draw_text(&mut x, y as i32 + 1, &number, GUTTER, font);

        let color = line_color(line);
        let mut pen = TEXT_LEFT as i32;
        let mut column = 0;
        for character in line.chars() {
            if column < editor.scroll_col {
                column += 1;
                continue;
            }
            if pen >= TEXT_RIGHT as i32 {
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
            && pen < TEXT_RIGHT as i32
        {
            surface.fill(Rect::new(pen, y as i32, 2, ROW as i32), CURSOR);
        }
    }

    if editor.text.is_empty() && editor.scroll_line == 0 {
        let mut x = TEXT_LEFT as i32;
        surface.draw_text(&mut x, BODY_TOP as i32 + 1, "（空文档）", MUTED, font);
    }
}

fn draw_output(model: &Model, font: &Font, surface: &Surface) {
    surface.fill(
        Rect::new(
            SIDEBAR as i32,
            OUTPUT_TOP as i32,
            (WIDTH - SIDEBAR) as i32,
            OUTPUT as i32,
        ),
        OUTPUT_BG,
    );
    let mut x = SIDEBAR as i32 + PAD as i32;
    surface.draw_text(&mut x, OUTPUT_TOP as i32 + 4, "输出", MUTED, font);

    let rows = (OUTPUT - 24) / 16;
    let start = model.output.len().saturating_sub(rows);
    for (row, line) in model.output[start..].iter().enumerate() {
        let y = OUTPUT_TOP + 22 + row * 16;
        let color = if line.contains("失败") || line.contains("不可用") {
            WARN
        } else if line.starts_with("  ") {
            MUTED
        } else {
            TEXT
        };
        let mut x = SIDEBAR as i32 + PAD as i32;
        draw_clipped(
            surface,
            font,
            &mut x,
            y as i32,
            (WIDTH - PAD) as i32,
            color,
            line,
        );
    }
}

fn draw_footer(model: &Model, font: &Font, surface: &Surface) {
    surface.fill(
        Rect::new(0, (HEIGHT - FOOTER) as i32, WIDTH as i32, FOOTER as i32),
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
        (HEIGHT - FOOTER) as i32 + 3,
        (WIDTH - PAD) as i32,
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
