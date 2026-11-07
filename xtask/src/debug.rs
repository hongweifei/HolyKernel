//! 带 GDB 存根启动 QEMU。

use std::fs;

use clap::Args;

use crate::failure::{Failure, Result};
use crate::image::binary_path;
use crate::target::Selection;
use crate::workspace;

#[derive(Debug, Args)]
pub struct DebugArguments {
    /// 使用 release 配置
    #[arg(long)]
    pub release: bool,
    /// GDB 远程端口
    #[arg(long, default_value_t = 1234)]
    pub port: u16,
    /// 启动图形桌面
    #[arg(long)]
    pub ui: bool,
    /// QEMU 内存与 CPU 配置
    #[command(flatten)]
    pub machine: crate::qemu::MachineArguments,
}

/// 构建镜像并以 `-S` 启动 QEMU，等待 GDB 连接。
pub fn run(arguments: DebugArguments) -> Result<()> {
    let cmdline = if arguments.ui {
        "desktop vfs.fs=holyfs"
    } else {
        "shell-interactive"
    };
    let iso = crate::image::run(Selection::X86_64Qemu, arguments.release, Some(cmdline))?;
    let disk = if arguments.ui {
        let disk = workspace::root().join("build/run-holy.img");
        crate::disk::ensure_holyfs(&disk)?;
        disk
    } else {
        let disk = workspace::root().join("build/run-disk.img");
        crate::disk::ensure_ext4(&disk)?;
        disk
    };

    let kernel = binary_path("hk-x86_64", arguments.release);
    let script = workspace::root().join("build/holy.gdb");
    let script_text = format!(
        "set pagination off\n\
         set confirm off\n\
         set architecture i386:x86-64\n\
         file {}\n\
         target remote :{}\n\
         hbreak kmain\n\
         continue\n",
        kernel.display(),
        arguments.port
    );
    fs::write(&script, script_text)
        .map_err(|error| Failure::new(format!("写入 {} 失败：{error}", script.display())))?;

    println!("GDB 脚本：{}", script.display());
    println!("在另一终端运行：gdb -x {}", script.display());
    println!("QEMU 已暂停，等待 GDB 连接 127.0.0.1:{}", arguments.port);
    crate::qemu::run_with(
        &iso,
        &disk,
        arguments.ui,
        &arguments.machine.machine(),
        Some(arguments.port),
    )
}
