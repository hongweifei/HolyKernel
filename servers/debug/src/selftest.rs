//! Debug 自检：逐页渲染、异常符号化与根服务符号查询。

use alloc::format;

use hk_font::Font;
use hk_proto::display::{DISPLAY_SHUTDOWN, Window};
use hk_server_api::{FAULT_EVENT, exit, fork, recv_data_try, root, send, write, yield_now};

use crate::pages::Page;
use crate::show;

/// 解析命令行；返回 `Some(fault_test)` 表示进入自检。
pub(crate) fn selected(cmdline: &str) -> Option<bool> {
    let fault_test = cmdline.contains("debug-crash");
    (fault_test || cmdline.contains("debug-test")).then_some(fault_test)
}

/// 运行脚本化自检并退出。
pub(crate) fn run(
    endpoint: u64,
    display: u64,
    font: &Font,
    window: &Window,
    fault_test: bool,
) -> ! {
    let base = u64::from_str_radix(env!("HK_IMAGE_BASE").trim_start_matches("0x"), 16).unwrap_or(0);
    for page in Page::ALL {
        let text = page.fetch();
        show(page, &text, 0, font, window);
        write(b"hk-server-debug: view ");
        write(page.slug().as_bytes());
        write(b"\n");
        if page == Page::Overview
            && let Some(arena) = hk_server_api::system_arena_info()
        {
            write(b"hk-server-debug: system-arena ");
            hk_server_api::write_u64(arena.regions);
            write(b"\n");
        }
    }
    if fault_test {
        let child = fork();
        if child == 0 {
            // SAFETY: 自检子任务故意触发非法指令，验证异常记录与符号化。
            unsafe {
                core::arch::asm!("ud2", options(noreturn));
            }
        }
        let mut fault = Page::Fault.fetch();
        let mut buffer = [0_u8; 2048];
        for _ in 0..100_000 {
            let (label, _word, _sender, length) = recv_data_try(endpoint, &mut buffer);
            if label == FAULT_EVENT {
                hk_debug::ingest(label, &buffer[..length]);
            }
            fault = Page::Fault.fetch();
            if fault.contains("异常：") {
                break;
            }
            let _ = yield_now();
        }
        show(Page::Fault, &fault, 0, font, window);
        if fault.contains("[hk-server-debug]") {
            write(b"hk-server-debug: fault symbolized\n");
        } else {
            write(b"hk-server-debug: fault raw\n");
        }
    }
    let mut symbol = [0_u8; 256];
    if let Ok(length) = root::call(root::ROOT_OP_SYMBOLIZE, "0xffffffff80000000", &mut symbol) {
        write(b"hk-server-debug: symbol ");
        write(
            core::str::from_utf8(&symbol[..length])
                .unwrap_or("?")
                .trim_end()
                .as_bytes(),
        );
        write(b"\n");
    }
    if let Some((name, offset)) = hk_debug::lookup("hk-server-debug", base) {
        write(b"hk-server-debug: service symbol ");
        write(format!("{base:#018x} {name}+0x{offset:x}").as_bytes());
        write(b"\n");
    }
    write(b"hk-server-debug: done\n");
    let _ = send(display, DISPLAY_SHUTDOWN, 0);
    exit(0)
}
