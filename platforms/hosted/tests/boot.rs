//! Hosted 启动路径的端到端检查。

use std::process::Command;

fn launch(arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_hk-hosted"))
        .args(arguments)
        .output()
        .expect("启动 hk-hosted")
}

#[test]
fn boots_and_exits_successfully() {
    let output = launch(&["--deterministic", "--seed", "42", "--cpus", "4"]);

    assert!(output.status.success(), "退出码：{:?}", output.status);
    let report = String::from_utf8(output.stdout).expect("报告是 UTF-8");
    assert!(report.contains("启动信息校验通过"));
    assert!(report.contains("CPU: 4"));
}

#[test]
fn rejects_zero_cpu_count() {
    let output = launch(&["--cpus", "0"]);

    assert!(!output.status.success(), "零 CPU 应当启动失败");
}
