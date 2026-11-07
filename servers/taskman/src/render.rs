//! 任务管理器窗口绘制：进程表格与 CPU/内存性能曲线。

use alloc::format;
use alloc::string::String;

use hk_canvas::{Rect, Surface, text_width};
use hk_font::Font;

use crate::model::{HISTORY, MAX_CORES, Page, PerfSample, SortKey, TaskRow};

/// 初始窗口宽度。
pub(crate) const WIDTH: usize = 860;
/// 初始窗口高度。
pub(crate) const HEIGHT: usize = 560;
/// 行高。
const LINE_HEIGHT: usize = 22;
/// 页面留白。
const MARGIN: i32 = 16;
/// 顶部标签基线。
const TAB_Y: i32 = 12;
/// 进程页表头基线。
const HEADER_Y: i32 = 76;
/// 进程页正文首行基线。
const ROW_TOP: usize = 100;
/// 底部状态栏高度。
const FOOTER_HEIGHT: usize = 26;

/// 背景色。
const BACKGROUND: u32 = 0x0010_1620;
/// 面板底色。
const PANEL: u32 = 0x0018_2028;
/// 行底色。
const ZEBRA: u32 = 0x0014_1C26;
/// 选中行底色。
const SELECT: u32 = 0x0024_3444;
/// 强调色。
const ACCENT: u32 = 0x0040_A0E0;
/// 图形填充色。
const GRAPH_FILL: u32 = 0x0028_5A78;
/// 图形网格色。
const GRID: u32 = 0x0024_3038;
/// 进度条轨道色。
const TRACK: u32 = 0x0024_3038;
/// 标题颜色。
const TITLE_INK: u32 = 0x0080_C0FF;
/// 正文颜色。
const INK: u32 = 0x00C8_DCD0;
/// 次要文字颜色。
const MUTED: u32 = 0x0070_8898;
/// 分隔线颜色。
const LINE: u32 = 0x0028_3038;
/// 状态栏背景色。
const FOOTER_BG: u32 = 0x0018_2028;
/// 状态栏文字颜色。
const FOOTER_INK: u32 = 0x0068_8090;
/// 运行中。
const RUNNING: u32 = 0x0080_E0A0;
/// 就绪。
const RUNNABLE: u32 = 0x0080_C0FF;
/// 阻塞。
const BLOCKED: u32 = 0x00E8_C080;
/// 已退出。
const EXITED: u32 = 0x00F0_8080;

/// 进程页列左边界。
const PROC_X: [i32; 8] = [16, 60, 240, 304, 360, 510, 554, 590];
/// 进程页列宽。
const PROC_WIDTH: [i32; 8] = [44, 180, 64, 56, 150, 44, 36, 44];
/// 进程页列标题。
const PROC_TITLE: [&str; 8] = ["TCB", "任务", "状态", "CPU", "Arena", "句柄", "父", "端点"];

/// 绘制所需的只读快照。
pub(crate) struct View<'a> {
    /// 当前页面。
    pub page: Page,
    /// 最新性能快照。
    pub sample: &'a PerfSample,
    /// 总 CPU 占用（千分比）。
    pub cpu_usage: u64,
    /// CPU 占用历史（千分比）。
    pub cpu_history: &'a [u64],
    /// 每 CPU 占用（千分比）。
    pub core_usage: &'a [u64],
    /// 内存占用历史（字节）。
    pub memory_history: &'a [u64],
    /// 当前排序键。
    pub sort: SortKey,
    /// 选中行下标。
    pub selected: usize,
    /// 首行滚动偏移。
    pub scroll: usize,
    /// 是否自动刷新。
    pub auto_refresh: bool,
}

/// 正文可见行数。
pub(crate) fn visible_rows(height: usize) -> usize {
    let footer_top = height.saturating_sub(FOOTER_HEIGHT);
    footer_top
        .saturating_sub(ROW_TOP)
        .checked_div(LINE_HEIGHT)
        .unwrap_or(0)
        .max(1)
}

/// 标签点击命中。
pub(crate) fn tab_hit(x: u32, y: u32) -> Option<Page> {
    if !(4..36).contains(&y) {
        return None;
    }
    match x {
        12..=83 => Some(Page::Processes),
        88..=159 => Some(Page::Performance),
        _ => None,
    }
}

/// 进程表格行点击映射到的行下标。
pub(crate) fn row_hit(y: u32, scroll: usize, rows: usize) -> Option<usize> {
    let top = ROW_TOP as u32;
    if y < top {
        return None;
    }
    let index = scroll + ((y - top) / LINE_HEIGHT as u32) as usize;
    (index < rows).then_some(index)
}

/// 把当前页面绘制到共享表面。
pub(crate) fn render(view: &View<'_>, width: usize, height: usize, surface: &Surface) {
    let width_i32 = width.min(i32::MAX as usize) as i32;
    let height_i32 = height.min(i32::MAX as usize) as i32;
    surface.fill(Rect::new(0, 0, width_i32, height_i32), BACKGROUND);
    surface.fill(Rect::new(0, 0, width_i32, 2), ACCENT);
    render_tabs(view.page, surface);

    match view.page {
        Page::Processes => render_processes(view, width_i32, height, surface),
        Page::Performance => render_performance(view, width_i32, height_i32, surface),
    }

    let footer_top = height_i32.saturating_sub(FOOTER_HEIGHT as i32);
    surface.fill(
        Rect::new(0, footer_top, width_i32, FOOTER_HEIGHT as i32),
        FOOTER_BG,
    );
    let hint = match view.page {
        Page::Processes => {
            "↑/↓ 选择  PgUp/PgDn 翻页  r 刷新  空格 暂停  n/a/s/h/c/t 排序  Tab 性能  q 退出"
        }
        Page::Performance => "r 刷新  空格 暂停  Tab 进程  q 退出",
    };
    let status = format!(
        "{} · 排序 {}",
        if view.auto_refresh {
            "自动刷新 1s"
        } else {
            "已暂停"
        },
        view.sort.title(),
    );
    let table = hk_canvas::default_monospace();
    let status_width = text_width(table, &status);
    let hint_limit = (width_i32 - MARGIN * 2 - status_width - 16).max(0);
    let mut pen = MARGIN;
    surface.draw_text_ellipsis(
        &mut pen,
        footer_top + 5,
        hint,
        hint_limit,
        FOOTER_INK,
        table,
    );
    let mut pen = (width_i32 - MARGIN - status_width).max(MARGIN);
    surface.draw_text(&mut pen, footer_top + 5, &status, FOOTER_INK, table);
}

fn render_tabs(active: Page, surface: &Surface) {
    let heading = hk_canvas::default_font();
    for (page, x) in [(Page::Processes, 20), (Page::Performance, 96)] {
        let color = if page == active { TITLE_INK } else { MUTED };
        let mut pen = x;
        surface.draw_text(&mut pen, TAB_Y, page.title(), color, heading);
        if page == active {
            surface.fill(Rect::new(x, 34, pen - x, 2), ACCENT);
        }
    }
}

fn render_processes(view: &View<'_>, width: i32, height: usize, surface: &Surface) {
    let table = hk_canvas::default_monospace();
    let summary = format!(
        "任务 {} · 就绪 {} · CPU {} · System {} / {} MiB · Arena {} MiB",
        view.sample.tasks,
        view.sample.runnable,
        percent_text(view.cpu_usage),
        mib_text(view.sample.memory_used()),
        mib_text(view.sample.memory_total()),
        mib_text(view.sample.arena_used()),
    );
    let mut pen = MARGIN;
    surface.draw_text(&mut pen, 52, &summary, MUTED, table);
    surface.fill(Rect::new(MARGIN, 62, width - MARGIN * 2, 1), LINE);

    for (index, title) in PROC_TITLE.iter().enumerate() {
        let mut pen = PROC_X[index];
        surface.draw_text_ellipsis(&mut pen, HEADER_Y, title, PROC_WIDTH[index], MUTED, table);
    }
    surface.fill(
        Rect::new(MARGIN, HEADER_Y + 16, width - MARGIN * 2, 1),
        LINE,
    );

    let visible = visible_rows(height);
    if view.sample.rows.is_empty() {
        let mut pen = MARGIN;
        surface.draw_text(&mut pen, ROW_TOP as i32, "暂无任务", MUTED, table);
    }
    for (index, row) in view
        .sample
        .rows
        .iter()
        .enumerate()
        .skip(view.scroll)
        .take(visible)
    {
        let y = ROW_TOP as i32 + (index - view.scroll) as i32 * LINE_HEIGHT as i32;
        draw_process_row(surface, row, y, width, table, index, index == view.selected);
    }
}

fn draw_process_row(
    surface: &Surface,
    row: &TaskRow,
    y: i32,
    width: i32,
    font: &Font,
    index: usize,
    selected: bool,
) {
    if selected {
        surface.fill(Rect::new(0, y - 3, width, LINE_HEIGHT as i32), SELECT);
        surface.fill(Rect::new(0, y - 3, 3, LINE_HEIGHT as i32), ACCENT);
    } else if index % 2 == 1 {
        surface.fill(Rect::new(0, y - 3, width, LINE_HEIGHT as i32), ZEBRA);
    }

    let parent = match row.parent {
        Some(parent) => format!("{parent}"),
        None => String::from("-"),
    };
    let memory = format!(
        "{}/{}K {}%",
        row.arena_used / 1024,
        row.arena_size / 1024,
        row.arena_percent()
    );

    cell(surface, 0, y, &format!("{}", row.tcb), MUTED, font);
    cell(
        surface,
        1,
        y,
        &row.name,
        if selected { TITLE_INK } else { INK },
        font,
    );
    cell(surface, 2, y, &row.state, state_color(&row.state), font);
    cell(
        surface,
        3,
        y,
        &format!("{}%", row.cpu_percent),
        usage_color(row.cpu_percent.saturating_mul(10)),
        font,
    );
    cell(surface, 4, y, &memory, MUTED, font);
    cell(surface, 5, y, &format!("{}", row.handles), INK, font);
    cell(surface, 6, y, &parent, MUTED, font);
    cell(surface, 7, y, &format!("{}", row.endpoint), MUTED, font);
}

fn render_performance(view: &View<'_>, width: i32, height: i32, surface: &Surface) {
    let table = hk_canvas::default_monospace();
    let heading = hk_canvas::default_font();
    let big = hk_canvas::default_font_scaled(2);

    let cpu_panel = Rect::new(MARGIN, 48, width - MARGIN * 2, 210);
    surface.fill(cpu_panel, PANEL);
    let mut pen = cpu_panel.x + 12;
    surface.draw_text(&mut pen, cpu_panel.y + 10, "CPU", MUTED, heading);
    let cores = format!("{} 个逻辑 CPU", view.sample.cpus.min(MAX_CORES));
    let mut pen = cpu_panel.x + 52;
    surface.draw_text(&mut pen, cpu_panel.y + 10, &cores, MUTED, table);
    let usage = percent_text(view.cpu_usage);
    let usage_width = text_width(big, &usage);
    let mut pen = cpu_panel.right() - 12 - usage_width;
    surface.draw_text(&mut pen, cpu_panel.y + 2, &usage, ACCENT, big);
    let cpu_graph = Rect::new(
        cpu_panel.x + 12,
        cpu_panel.y + 42,
        cpu_panel.width - 24,
        cpu_panel.height - 54,
    );
    draw_graph(surface, cpu_graph, view.cpu_history, 1000, GRAPH_FILL);

    let core_panel = Rect::new(MARGIN, 266, width - MARGIN * 2, 100);
    surface.fill(core_panel, PANEL);
    let mut pen = core_panel.x + 12;
    surface.draw_text(&mut pen, core_panel.y + 8, "每核", MUTED, table);
    let shown = view.sample.cpus.min(8).min(MAX_CORES);
    let columns = if shown > 4 { 2 } else { 1 };
    let rows = shown.max(1).div_ceil(columns);
    let cell_width = (core_panel.width - 24) / columns as i32;
    for cpu in 0..shown {
        let column = cpu / rows.max(1);
        let row = cpu % rows.max(1);
        let x = core_panel.x + 12 + column as i32 * cell_width;
        let y = core_panel.y + 24 + row as i32 * 18;
        let mut pen = x;
        surface.draw_text(&mut pen, y, &format!("CPU{cpu}"), MUTED, table);
        let bar_x = x + 46;
        let bar_width = (cell_width - 46 - 52).max(8);
        let usage = view.core_usage.get(cpu).copied().unwrap_or(0);
        draw_bar(surface, bar_x, y + 4, bar_width, usage);
        let mut pen = bar_x + bar_width + 8;
        surface.draw_text(&mut pen, y, &format!("{}%", usage / 10), MUTED, table);
    }
    if view.sample.cpus > shown {
        let mut pen = core_panel.right() - 12 - text_width(table, "…");
        surface.draw_text(&mut pen, core_panel.bottom() - 20, "…", MUTED, table);
    }

    let memory_panel = Rect::new(MARGIN, 374, width - MARGIN * 2, 130);
    surface.fill(memory_panel, PANEL);
    let mut pen = memory_panel.x + 12;
    surface.draw_text(&mut pen, memory_panel.y + 10, "内存", MUTED, heading);
    let used = view.sample.memory_used();
    let total = view.sample.memory_total();
    let percent = if total == 0 {
        0
    } else {
        used.saturating_mul(1000) / total
    };
    let memory_text = format!(
        "{} / {} MiB  ({})  峰值 {} MiB",
        mib_text(used),
        mib_text(total),
        percent_text(percent),
        mib_text(view.sample.system.peak),
    );
    let memory_width = text_width(heading, &memory_text);
    let mut pen = memory_panel.right() - 12 - memory_width;
    surface.draw_text(&mut pen, memory_panel.y + 8, &memory_text, INK, heading);
    let categories = format!(
        "heap {} · 页表 {} · COW {} · 恢复 {} · 镜像 {} · DMA {} · 其他 {} MiB",
        mib_text(view.sample.system.heap),
        mib_text(view.sample.system.page_table),
        mib_text(view.sample.system.cow),
        mib_text(view.sample.system.restore),
        mib_text(view.sample.system.image),
        mib_text(view.sample.system.dma),
        mib_text(view.sample.system.other),
    );
    let mut pen = memory_panel.x + 12;
    surface.draw_text_ellipsis(
        &mut pen,
        memory_panel.y + 26,
        &categories,
        memory_panel.width - 24,
        MUTED,
        table,
    );
    let memory_graph = Rect::new(
        memory_panel.x + 12,
        memory_panel.y + 44,
        memory_panel.width - 24,
        memory_panel.height - 56,
    );
    draw_graph(
        surface,
        memory_graph,
        view.memory_history,
        total.max(1),
        GRAPH_FILL,
    );

    let handles: u64 = view.sample.rows.iter().map(|row| row.handles).sum();
    let stats = format!(
        "任务 {} · 就绪 {} · 句柄 {} · tick {} · TSC {} MHz",
        view.sample.tasks,
        view.sample.runnable,
        handles,
        view.sample.ticks,
        view.sample.tsc_hz / 1_000_000,
    );
    let mut pen = MARGIN;
    surface.draw_text_ellipsis(&mut pen, 514, &stats, width - MARGIN * 2, MUTED, table);
    let _ = height;
}

fn draw_graph(surface: &Surface, rect: Rect, history: &[u64], maximum: u64, color: u32) {
    surface.fill(rect, BACKGROUND);
    for step in 0..=4 {
        let y = rect.y + (rect.height - 1) * step / 4;
        surface.fill(Rect::new(rect.x, y, rect.width, 1), GRID);
    }
    if history.is_empty() || maximum == 0 {
        return;
    }
    let slot = (rect.width as usize / HISTORY).max(1);
    let start = HISTORY.saturating_sub(history.len());
    for (index, value) in history.iter().enumerate() {
        let x = rect.x + ((start + index) * slot) as i32;
        if x >= rect.right() {
            break;
        }
        let scaled = (*value).min(maximum);
        let height = (scaled.saturating_mul(rect.height as u64 - 2) / maximum) as i32;
        if height <= 0 {
            continue;
        }
        let width = slot.saturating_sub(1).max(1) as i32;
        let bar = Rect::new(x, rect.bottom() - 1 - height, width, height);
        surface.fill(bar, color);
        surface.fill(Rect::new(x, rect.bottom() - 1 - height, width, 1), ACCENT);
    }
}

fn draw_bar(surface: &Surface, x: i32, y: i32, width: i32, usage: u64) {
    surface.fill(Rect::new(x, y, width, 8), TRACK);
    let filled = (width as u64 * usage.min(1000) / 1000) as i32;
    if filled > 0 {
        surface.fill(Rect::new(x, y, filled, 8), usage_color(usage));
    }
}

fn cell(surface: &Surface, column: usize, y: i32, text: &str, color: u32, font: &Font) {
    let mut pen = PROC_X[column];
    surface.draw_text_ellipsis(&mut pen, y, text, PROC_WIDTH[column], color, font);
}

fn state_color(state: &str) -> u32 {
    match state {
        "running" => RUNNING,
        "runnable" => RUNNABLE,
        "blocked" => BLOCKED,
        "exited" => EXITED,
        _ => INK,
    }
}

fn usage_color(permille: u64) -> u32 {
    if permille >= 800 {
        EXITED
    } else if permille >= 500 {
        BLOCKED
    } else {
        INK
    }
}

fn percent_text(permille: u64) -> String {
    format!("{}.{}%", permille / 10, permille % 10)
}

fn mib_text(bytes: u64) -> String {
    let tenths = bytes.saturating_mul(10) / (1024 * 1024);
    format!("{}.{}", tenths / 10, tenths % 10)
}
