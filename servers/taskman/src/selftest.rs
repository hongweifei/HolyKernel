//! Taskman 自检：空闲采样、两页渲染与状态输出。

use hk_proto::display::{DISPLAY_SHUTDOWN, Window};
use hk_server_api::{exit, send, sleep_ticks, write, write_u64};

use crate::model::{Page, REFRESH_TICKS};
use crate::{App, show};

/// 命令行是否选择自检。
pub(crate) fn selected(cmdline: Option<&str>) -> bool {
    cmdline.is_some_and(|line| line.contains("taskman-test"))
}

/// 运行脚本化自检并退出。
pub(crate) fn run(app: &mut App, window: &Window, display: u64) -> ! {
    // 空闲采样一个刷新周期，验证 CPU 占用率不会因为轮询而持续打满。
    let _ = sleep_ticks(REFRESH_TICKS);
    app.refresh();
    app.switch_page(Page::Processes);
    show(app, window);
    app.switch_page(Page::Performance);
    show(app, window);
    log_test(app, window.window());
    let _ = send(display, DISPLAY_SHUTDOWN, 0);
    exit(0)
}

/// 输出自检标记。
fn log_test(app: &App, window: u64) {
    write(b"hk-server-taskman: ready ");
    write_u64(window);
    write(b"\n");
    write(b"hk-server-taskman: cpus ");
    write_u64(app.sample.cpus as u64);
    write(b"\n");
    write(b"hk-server-taskman: tasks ");
    write_u64(app.sample.tasks);
    write(b"\n");
    write(b"hk-server-taskman: rows ");
    write_u64(app.sample.rows.len() as u64);
    write(b"\n");
    write(b"hk-server-taskman: cpu ");
    write_u64(app.cpu_usage);
    write(b"\n");
    for row in app.rows().iter().take(8) {
        write(b"hk-server-taskman: task ");
        write(row.name.as_bytes());
        write(b" ");
        write(row.state.as_bytes());
        write(b"\n");
    }
    let busy: u64 = app.sample.cpu_busy.iter().sum();
    write(b"hk-server-taskman: busy ");
    write_u64(busy);
    write(b"\n");
    write(b"hk-server-taskman: memory ");
    write_u64(app.sample.memory_used());
    write(b" ");
    write_u64(app.sample.memory_total());
    write(b" ");
    write_u64(app.sample.system.peak);
    write(b" ");
    write_u64(app.sample.system.soft);
    write(b" ");
    write_u64(app.sample.system.span);
    write(b" ");
    write_u64(app.sample.system.windows);
    write(b"\n");
    write(b"hk-server-taskman: frame-bank ");
    write_u64(app.sample.frame_bank.used);
    write(b" ");
    write_u64(app.sample.frame_bank.total);
    write(b" ");
    write_u64(app.sample.frame_bank.peak);
    write(b" ");
    write_u64(app.sample.frame_bank.free);
    write(b"\n");
    write(b"hk-server-taskman: capacity-kinds ");
    write_u64(app.sample.frame_bank.system);
    write(b" ");
    write_u64(app.sample.frame_bank.user);
    write(b" ");
    write_u64(app.sample.frame_bank.dma);
    write(b" ");
    write_u64(app.sample.frame_bank.gpu);
    write(b" ");
    write_u64(app.sample.frame_bank.numa);
    write(b"\n");
    write(b"hk-server-taskman: system-arena ");
    write_u64(app.sample.system_arena.regions);
    write(b" ");
    write_u64(app.sample.system_arena.used);
    write(b"\n");
    write(b"hk-server-taskman: render ok\n");
    write(b"hk-server-taskman: done\n");
}
