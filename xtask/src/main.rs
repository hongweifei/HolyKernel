//! HolyKernel 构建、运行、测试与校验任务。
//!
//! ```text
//! cargo xtask build --target hosted
//! cargo xtask build --target x86_64-qemu --release
//! cargo xtask run   --target hosted
//! cargo xtask test  --target hosted
//! cargo xtask lint
//! cargo xtask docs
//! cargo xtask budget
//! cargo xtask inspect --service hk-server-demo --symbols
//! cargo xtask symbolize 0xffffffff80018bc4
//! cargo xtask debug
//! ```

mod budget;
mod build;
mod cargo;
mod catalog;
mod debug;
mod deps;
mod disk;
mod docs;
mod failure;
mod image;
mod inspect;
mod lint;
mod native;
mod qemu;
mod run;
mod smoke;
mod symbolize;
mod target;
mod test;
mod workspace;

use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use crate::failure::Result;
use crate::target::Selection;

#[derive(Debug, Parser)]
#[command(name = "xtask", about = "HolyKernel 构建与校验任务", version)]
struct Cli {
    #[command(subcommand)]
    task: Task,
}

#[derive(Debug, Subcommand)]
enum Task {
    /// 构建目标
    Build(BuildArguments),
    /// 构建可启动镜像
    Image(ImageArguments),
    /// 运行目标
    Run(RunArguments),
    /// 运行测试
    Test(TestArguments),
    /// 启动 QEMU 并校验串口输出
    Smoke(SmokeArguments),
    /// 查看原生镜像的 Task、Region、能力与符号
    Inspect(inspect::InspectArguments),
    /// 用内核与服务符号离线解析地址
    Symbolize(symbolize::SymbolizeArguments),
    /// 带 GDB 存根启动 QEMU
    Debug(debug::DebugArguments),
    /// 运行格式与静态检查
    Lint,
    /// 检查 crate 依赖方向
    Deps,
    /// 生成 rustdoc 并校验文档
    Docs,
    /// 统计内核代码行数
    Budget,
    /// 创建运行磁盘 build/run-disk.img
    Disk(DiskArguments),
}

#[derive(Debug, Args)]
struct DiskArguments {
    /// 创建 H: 卷桌面磁盘 build/run-holy.img
    #[arg(long)]
    holy: bool,
}

#[derive(Debug, Args)]
struct BuildArguments {
    /// 构建目标
    #[arg(long, value_enum, default_value = "hosted")]
    target: Selection,
    /// 使用 release 配置
    #[arg(long)]
    release: bool,
}

#[derive(Debug, Args)]
struct ImageArguments {
    /// 镜像目标
    #[arg(long, value_enum, default_value = "x86_64-qemu")]
    target: Selection,
    /// 使用 release 配置
    #[arg(long)]
    release: bool,
    /// 内核启动命令行；默认 shell-interactive
    #[arg(long)]
    cmdline: Option<String>,
}

#[derive(Debug, Args)]
struct RunArguments {
    /// 运行目标
    #[arg(long, value_enum, default_value = "hosted")]
    target: Selection,
    /// 使用 release 配置
    #[arg(long)]
    release: bool,
    /// 启动图形桌面（Display/Console/Input/HolyDoc）
    #[arg(long)]
    ui: bool,
    /// 传给目标程序的参数
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    arguments: Vec<String>,
}

#[derive(Debug, Args)]
struct TestArguments {
    /// 测试目标
    #[arg(long, value_enum, default_value = "hosted")]
    target: Selection,
}

#[derive(Debug, Args)]
struct SmokeArguments {
    /// 冒烟目标
    #[arg(long, value_enum, default_value = "x86_64-qemu")]
    target: Selection,
    /// 使用 release 配置
    #[arg(long)]
    release: bool,
    /// 冒烟模式
    #[arg(long, value_enum, default_value = "boot")]
    mode: smoke::SmokeMode,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match execute(cli.task) {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            eprintln!("xtask：{failure}");
            ExitCode::FAILURE
        }
    }
}

fn execute(task: Task) -> Result<()> {
    match task {
        Task::Build(arguments) => build::run(arguments.target, arguments.release),
        Task::Image(arguments) => {
            let cmdline = arguments.cmdline.as_deref().unwrap_or("shell-interactive");
            image::run(arguments.target, arguments.release, Some(cmdline)).map(|_| ())
        }
        Task::Run(arguments) => run::run(
            arguments.target,
            arguments.release,
            &arguments.arguments,
            arguments.ui,
        ),
        Task::Test(arguments) => test::run(arguments.target),
        Task::Smoke(arguments) => smoke::run(arguments.target, arguments.release, arguments.mode),
        Task::Inspect(arguments) => inspect::run(arguments),
        Task::Symbolize(arguments) => symbolize::run(arguments),
        Task::Debug(arguments) => debug::run(arguments),
        Task::Lint => lint::run(),
        Task::Deps => deps::run(),
        Task::Docs => docs::run(),
        Task::Budget => budget::run(),
        Task::Disk(arguments) => {
            let path = if arguments.holy {
                let path = workspace::root().join("build/run-holy.img");
                disk::ensure_holyfs(&path)?;
                path
            } else {
                let path = workspace::root().join("build/run-disk.img");
                disk::ensure_ext4(&path)?;
                path
            };
            println!("{}", path.display());
            Ok(())
        }
    }
}
