//! HolyKernel Hosted 模拟器启动入口。
//!
//! 启动平台、打印启动信息、校验 BootInfo，然后按退出协议结束进程：
//!
//! ```text
//! cargo xtask run --target hosted
//! cargo xtask run --target hosted -- --deterministic --seed 42 --cpus 4
//! ```

use std::fmt::Write;

use clap::Parser;
use hk_hal::{BootInfo, Platform};
use hk_platform_hosted::{HostedConfig, HostedPlatform};
use hk_test::ExitCode;

/// Hosted 模拟器启动参数。
#[derive(Debug, Parser)]
#[command(name = "hk-hosted", about = "HolyKernel Hosted 模拟器", version)]
struct Arguments {
    /// 逻辑 CPU 数量。
    #[arg(long, default_value_t = 1)]
    cpus: usize,

    /// 确定性模拟的随机种子。
    #[arg(long, default_value_t = 0)]
    seed: u64,

    /// 使用虚拟时钟，按显式推进的时间执行。
    #[arg(long)]
    deterministic: bool,
}

fn main() {
    let arguments = Arguments::parse();
    let config = HostedConfig::new(arguments.cpus, arguments.seed, arguments.deterministic);
    let platform = HostedPlatform::boot(config);
    let boot = platform.boot_info();

    platform.console().write_str(&describe(&platform, &boot));

    match platform.init(&boot) {
        Ok(()) => {
            platform.console().write_str("启动信息校验通过\n");
            hk_test::exit(ExitCode::Success);
        }
        Err(error) => {
            let mut report = String::new();
            writeln!(report, "启动失败：{error}").expect("格式化启动失败报告");
            platform.console().write_str(&report);
            hk_test::exit(ExitCode::Failure(1));
        }
    }
}

/// 生成启动信息报告。
fn describe(platform: &HostedPlatform, boot: &BootInfo) -> String {
    let mut report = String::new();
    let config = platform.config();

    writeln!(report, "HolyKernel hosted").expect("格式化启动报告");
    writeln!(report, "  平台: {}", platform.name()).expect("格式化启动报告");
    writeln!(report, "  架构: {}", boot.arch).expect("格式化启动报告");
    writeln!(report, "  CPU: {}", platform.cpu_count()).expect("格式化启动报告");
    writeln!(
        report,
        "  时钟: {}",
        if config.deterministic {
            "确定性虚拟时钟"
        } else {
            "系统单调时钟"
        }
    )
    .expect("格式化启动报告");
    writeln!(report, "  随机种子: {}", config.seed).expect("格式化启动报告");
    writeln!(report, "  内存 map:").expect("格式化启动报告");
    for region in boot.memory_map {
        writeln!(
            report,
            "    {:#012x} + {:#010x}  {}",
            region.base.get(),
            region.len,
            region.kind.name()
        )
        .expect("格式化启动报告");
    }
    writeln!(
        report,
        "  内核镜像: {:#012x} + {:#010x}",
        boot.kernel_image.base.get(),
        boot.kernel_image.len
    )
    .expect("格式化启动报告");
    match boot.direct_map {
        Some(direct_map) => {
            writeln!(
                report,
                "  direct map: {:#012x} -> {:#018x} + {:#010x}",
                direct_map.phys_base.get(),
                direct_map.virt_base.get(),
                direct_map.len
            )
            .expect("格式化启动报告");
        }
        None => {
            writeln!(report, "  direct map: 无").expect("格式化启动报告");
        }
    }
    writeln!(report, "  服务器模块: {}", boot.server_modules.len()).expect("格式化启动报告");

    report
}
