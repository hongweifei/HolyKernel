//! 在 QEMU 中运行镜像。

use std::path::Path;
use std::process::Command;

use clap::Args;

use crate::failure::{Failure, Result};

/// `isa-debug-exit` 的成功退出码：`(0 << 1) | 1`。
const QEMU_SUCCESS: i32 = 1;

/// QEMU 机器资源参数。
#[derive(Clone, Debug, Args)]
pub struct MachineArguments {
    /// CPU 核数
    #[arg(long, visible_alias = "cpu", default_value_t = 1, value_parser = parse_cpus)]
    pub cpus: u32,
    /// 内存大小，支持 K/M/G/T 后缀；默认 256M
    #[arg(long, default_value = "256M", value_parser = parse_ram)]
    pub ram: String,
}

impl MachineArguments {
    /// 转换为 QEMU 机器参数。
    pub fn machine(&self) -> Machine {
        Machine {
            ram: self.ram.clone(),
            cpus: self.cpus,
        }
    }
}

/// QEMU 机器资源。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Machine {
    /// 内存大小，已由 [`parse_ram`] 规范化。
    pub ram: String,
    /// CPU 核数。
    pub cpus: u32,
}

impl Default for Machine {
    fn default() -> Self {
        Self {
            ram: String::from("256M"),
            cpus: 1,
        }
    }
}

/// 校验并规范化内存大小；保留 QEMU 支持的 K/M/G/T 后缀。
fn parse_ram(value: &str) -> std::result::Result<String, String> {
    let text = value.trim();
    if text.is_empty() {
        return Err(String::from("内存大小不能为空"));
    }
    let (number, suffix) = match text.char_indices().next_back() {
        Some((index, character)) if character.is_ascii_alphabetic() => {
            (&text[..index], Some(character))
        }
        _ => (text, None),
    };
    let amount: u64 = number
        .parse()
        .map_err(|_| format!("无效的内存大小：{value}（示例：256M、1G）"))?;
    if amount == 0 {
        return Err(String::from("内存大小必须大于 0"));
    }
    let suffix = match suffix {
        Some(character) => {
            let upper = character.to_ascii_uppercase();
            if !matches!(upper, 'K' | 'M' | 'G' | 'T') {
                return Err(format!("无效的内存单位：{value}（支持 K/M/G/T）"));
            }
            Some(upper)
        }
        None => None,
    };
    Ok(match suffix {
        Some(unit) => format!("{amount}{unit}"),
        None => amount.to_string(),
    })
}

/// 校验 CPU 核数。
fn parse_cpus(value: &str) -> std::result::Result<u32, String> {
    let cpus: u32 = value
        .parse()
        .map_err(|_| format!("无效的 CPU 核数：{value}"))?;
    if cpus == 0 {
        return Err(String::from("CPU 核数必须大于 0"));
    }
    Ok(cpus)
}

/// 判断 KVM 是否可用：`/dev/kvm` 存在不代表当前进程能打开它（常见于 CI）。
fn kvm_available() -> bool {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/kvm")
        .is_ok()
}

/// 启动 QEMU 并等待内核退出。
pub fn run(iso: &Path, disk: &Path, ui: bool, machine: &Machine) -> Result<()> {
    run_with(iso, disk, ui, machine, None)
}

/// 启动 QEMU；`gdb` 为 GDB 远程端口时以 `-S` 暂停等待连接。
pub fn run_with(
    iso: &Path,
    disk: &Path,
    ui: bool,
    machine: &Machine,
    gdb: Option<u16>,
) -> Result<()> {
    let mut command = Command::new("qemu-system-x86_64");
    command
        .args(["-M", "q35"])
        .arg("-m")
        .arg(&machine.ram)
        .arg("-smp")
        .arg(machine.cpus.to_string())
        .arg("-cdrom")
        .arg(iso)
        .args(["-boot", "d", "-serial", "stdio", "-no-reboot"]);
    if kvm_available() {
        command.args(["-accel", "kvm", "-cpu", "host"]);
    }
    if !ui {
        command.args(["-display", "none"]);
    }
    if let Some(port) = gdb {
        command.arg("-gdb").arg(format!("tcp::{port}")).arg("-S");
    }
    command
        .arg("-drive")
        .arg(format!(
            "file={},if=none,id=disk,format=raw",
            disk.display()
        ))
        .args(["-device", "virtio-blk-pci,drive=disk"]);
    if ui {
        // 平板必须先于键盘：QEMU 的绝对指针事件交给最先注册的输入设备，
        // 键盘在前时指针事件会被丢掉，桌面上就看不到鼠标。
        command.args(["-device", "virtio-tablet-pci"]);
        command.args(["-device", "virtio-keyboard-pci"]);
    }
    let status = command
        .args(["-device", "isa-debug-exit,iobase=0xf4,iosize=0x04"])
        .status()
        .map_err(|error| Failure::new(format!("启动 qemu-system-x86_64 失败：{error}")))?;

    match status.code() {
        Some(QEMU_SUCCESS) => Ok(()),
        Some(code) => Err(Failure::new(format!("QEMU 退出码 {code}"))),
        None => Err(Failure::new("QEMU 被信号终止")),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_cpus, parse_ram};

    #[test]
    fn normalizes_ram_suffixes() {
        assert_eq!(parse_ram("256M").unwrap(), "256M");
        assert_eq!(parse_ram("1g").unwrap(), "1G");
        assert_eq!(parse_ram(" 512 ").unwrap(), "512");
        assert_eq!(parse_ram("64k").unwrap(), "64K");
    }

    #[test]
    fn rejects_invalid_ram() {
        assert!(parse_ram("").is_err());
        assert!(parse_ram("0M").is_err());
        assert!(parse_ram("128X").is_err());
        assert!(parse_ram("abc").is_err());
        assert!(parse_ram("1.5G").is_err());
    }

    #[test]
    fn validates_cpu_count() {
        assert_eq!(parse_cpus("4").unwrap(), 4);
        assert!(parse_cpus("0").is_err());
        assert!(parse_cpus("abc").is_err());
    }
}
