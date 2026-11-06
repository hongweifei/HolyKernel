//! Workbench 自检：文档树、打开、保存、构建、运行与调试。

use alloc::format;

use hk_font::Font;
use hk_server_api::display::DISPLAY_SHUTDOWN;
use hk_server_api::{exit, send, write, write_u64};

use crate::model::Model;

/// 自检文档内容。
const SAMPLE: &str = "@title(\"WorkbenchCheck\")\n@exec version\n@exec ticks\nWorkbench 编辑测试\n";

/// 运行脚本化自检并退出。
#[allow(clippy::too_many_arguments)]
pub(crate) fn run(
    model: &mut Model,
    font: &Font,
    pixels: *mut u32,
    endpoint: u64,
    display: u64,
    surface: u64,
    window: u64,
    width: usize,
    height: usize,
) -> ! {
    write(b"hk-server-workbench: selftest\n");

    if model.select_path("H:/Welcome") {
        crate::show(
            model, font, pixels, endpoint, display, surface, window, width, height,
        );
        write(b"hk-server-workbench: open H:/Welcome\n");
    } else {
        write(b"hk-server-workbench: open failed\n");
    }

    match model.create_document("WorkbenchCheck") {
        Ok(document) => {
            model.load_editor(document, "H:/WorkbenchCheck", "WorkbenchCheck", SAMPLE);
            model.save();
            let saved = model
                .read_document(document)
                .is_ok_and(|text| text == SAMPLE);
            write(if saved {
                b"hk-server-workbench: save ok\n".as_slice()
            } else {
                b"hk-server-workbench: save failed\n".as_slice()
            });

            let summary = model.build();
            write(b"hk-server-workbench: ");
            write(summary.as_bytes());
            write(b"\n");

            let executed = model.run();
            write(b"hk-server-workbench: run ");
            write_u64(executed as u64);
            write(b"\n");

            let debug = model.debug();
            write(b"hk-server-workbench: debug ");
            write_u64(debug as u64);
            write(b"\n");

            let purged = model.purge_document(document).is_ok();
            write(if purged {
                b"hk-server-workbench: purge ok\n".as_slice()
            } else {
                b"hk-server-workbench: purge failed\n".as_slice()
            });
        }
        Err(status) => {
            write(b"hk-server-workbench: create failed ");
            write_u64(u64::from(status));
            write(b"\n");
        }
    }

    if let Some(index) = model.path_index("H:/Welcome") {
        model.open(index);
    }
    model.log(&format!("自检完成：{} 项文档", model.entries.len()));
    crate::show(
        model, font, pixels, endpoint, display, surface, window, width, height,
    );
    write(b"hk-server-workbench: done\n");
    let _ = send(display, DISPLAY_SHUTDOWN, 0);
    exit(0)
}
