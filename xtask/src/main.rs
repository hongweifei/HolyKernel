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
//! ```

mod budget;
mod build;
mod cargo;
mod docs;
mod failure;
mod lint;
mod run;
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
    /// 运行目标
    Run(RunArguments),
    /// 运行测试
    Test(TestArguments),
    /// 运行格式与静态检查
    Lint,
    /// 生成 rustdoc 并校验文档
    Docs,
    /// 统计内核代码行数
    Budget,
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
struct RunArguments {
    /// 运行目标
    #[arg(long, value_enum, default_value = "hosted")]
    target: Selection,
    /// 使用 release 配置
    #[arg(long)]
    release: bool,
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
        Task::Run(arguments) => run::run(arguments.target, arguments.release, &arguments.arguments),
        Task::Test(arguments) => test::run(arguments.target),
        Task::Lint => lint::run(),
        Task::Docs => docs::run(),
        Task::Budget => budget::run(),
    }
}
