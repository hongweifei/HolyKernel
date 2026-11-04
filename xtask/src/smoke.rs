//! QEMU 冒烟测试：启动内核并检查串口输出。

mod qmp;

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use hk_block::{MemoryDisk, SECTOR_SIZE};
use hk_hal::CivilTime;
use hk_vfs::Backend;
use qmp::inject;

use crate::failure::{Failure, Result};
use crate::target::{Selection, Target};
use crate::workspace;

/// 等待内核退出的上限。
const TIMEOUT: Duration = Duration::from_secs(30);

/// 测试磁盘大小。
const TEST_DISK_BYTES: u64 = 64 * 1024 * 1024;

/// ext4 自检写入的标记内容。
const FS_MARKER: &[u8] = b"HolyKernel ext4 ok\n";

/// QEMU 用户网络中的宿主 TCP echo 端口。
const NET_TCP_PORT: u16 = 45678;

/// `isa-debug-exit` 的成功退出码：`(0 << 1) | 1`。
const QEMU_SUCCESS: i32 = 1;

/// 异常测试的退出码：`(70 << 1) | 1`。
const QEMU_EXCEPTION: i32 = 141;

/// 冒烟模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum SmokeMode {
    /// 正常启动并校验 BootInfo
    #[value(name = "boot")]
    Boot,
    /// 非法指令异常
    #[value(name = "ud2")]
    Ud2,
    /// 空指针写入触发页错误
    #[value(name = "page")]
    Page,
    /// 本地 APIC 定时器中断
    #[value(name = "timer")]
    Timer,
    /// 帧缓冲文本输出
    #[value(name = "fb")]
    Framebuffer,
    /// HolyShell 脚本化命令
    #[value(name = "shell")]
    Shell,
    /// 键盘输入驱动的交互式 HolyShell
    #[value(name = "input")]
    Input,
    /// 定时器抢占调度
    #[value(name = "preempt")]
    Preempt,
    /// 多 CPU MADT 本地 APIC 枚举
    #[value(name = "smp")]
    Smp,
    /// 块驱动退出后由监督循环重启并恢复 ext4
    #[value(name = "restart")]
    Restart,
    /// ext4 作为 VFS 主根挂载
    #[value(name = "ext4root")]
    Ext4Root,
    /// HolyFS 数据盘挂载
    #[value(name = "holyfs")]
    HolyFs,
    /// virtio-net 驱动探测与 MAC 输出
    #[value(name = "net")]
    Net,
}

impl SmokeMode {
    fn cmdline(self) -> Option<&'static str> {
        match self {
            Self::Boot => None,
            Self::Ud2 => Some("fault-test"),
            Self::Page => Some("page-fault-test"),
            Self::Timer => Some("timer-test"),
            Self::Framebuffer => Some("fb-test"),
            Self::Shell => Some(
                "shell-test help;focus hk-server-echo;tasks;start hk-server-device;tasks;handles;services;restart hk-server-demo;services;mounts;start hk-server-fault;start hk-server-fork;start hk-server-latency;tasks;trace;stats",
            ),
            Self::Input => Some("shell-interactive"),
            Self::Preempt => None,
            Self::Smp => Some("shell-test tasks;stats"),
            Self::Restart => Some("restart-test"),
            Self::Ext4Root => Some("vfs.root=ext4"),
            Self::HolyFs => Some("vfs.fs=holyfs"),
            Self::Net => Some("net-test net.dns=localhost shell-test net"),
        }
    }

    fn expected(self) -> &'static [&'static str] {
        match self {
            Self::Boot => &[
                "TSC 频率",
                "启动信息校验通过",
                "内核初始化耗时",
                "加载服务并进入用户态",
                "hk-server-demo running",
                "hk-server-echo 收到请求 21",
                "hk-server-demo 收到回复 42",
                "hk-server-demo arena ok",
                "hk-server-echo 收到句柄",
                "hk-server-demo 句柄转移 ok",
                "hk-server-echo 已发出通知",
                "hk-server-demo 通知等待 ok",
                "hk-server-device: 发现",
                "hk-server-virtio: virtio-blk sectors=131072",
                "hk-server-vfs: rtc ok",
                "hk-server-vfs: ext4 mounted",
                "hk-server-demo fs ok",
                "fs 1 MiB 读：",
                "hk-server-spin: done",
                "hk-server-spin: closed",
                "VFS 读写 ok",
                "调度 tick：",
                "启动耗时",
                "服务进入空闲",
            ],
            Self::Ud2 => &["内核异常：非法指令"],
            Self::Page => &["内核异常：页错误"],
            Self::Timer => &["定时器中断 5 次"],
            Self::Framebuffer => &["帧缓冲控制台: 80x36 字符"],
            Self::Shell => &[
                "HolyShell",
                "当前任务：hk-server-echo",
                "已在运行：hk-server-device",
                "任务表（7 项）",
                "父",
                "句柄",
                "Arena",
                "VSpace",
                "4/2048K",
                "Handle 计数",
                "重启",
                "已重启：hk-server-demo",
                "hk-server-demo running",
                "hk-server-demo 收到回复 42",
                "hk-server-demo 句柄转移 ok",
                "hk-server-demo 通知等待 ok",
                "用户异常：页错误",
                "hk-server-fault",
                "hk-server-fork: child 22",
                "hk-server-fork: parent 33",
                "hk-server-latency: avg",
                "trace（",
                "handle",
                "上下文切换：",
                "IPC call/reply：",
                "hk-server-demo",
                "hk-server-echo",
                "hk-server-device",
                "hk-server-spin",
                "尚无挂载",
            ],
            Self::Input => &[
                "IOAPIC 就绪",
                "HolyShell 交互模式",
                "hk> echo hello-ioapic",
                "hello-ioapic",
                "求值：boot.hsh",
                "脚本：boot.hsh",
            ],
            Self::Preempt => &["hk-server-demo 收到回复 42", "hk-server-spin: done"],
            Self::Smp => &[
                "本地 APIC：4 个",
                "AP 在线：3 个（启动 3，IPI 1 次）",
                "AP 空闲 IPI 唤醒完成，TLB shootdown",
                "CPU 调度：",
                "CPU1=",
            ],
            Self::Restart => &[
                "hk-server-vfs: rtc ok",
                "hk-server-vfs: ext4 mounted",
                "hk-server-demo fs ok",
                "hk-server-demo 驱动重启 ok",
                "fs 重连 ok",
                "fs 1 MiB 读：",
            ],
            Self::Ext4Root => &[
                "hk-server-vfs: ext4 mounted",
                "hk-server-demo fs ok",
                "fs 1 MiB 读：",
            ],
            Self::HolyFs => &[
                "hk-server-vfs: holyfs mounted",
                "hk-server-demo fs ok",
                "fs 1 MiB 读：",
            ],
            Self::Net => &[
                "hk-server-virtio-net: mac=",
                "hk-server-net: ready",
                "hk-server-net: ping ok",
                "hk-server-net: tcp ok",
                "hk-server-net: dns ok 127.0.0.1",
                "hk-server-netstat: mac=",
                "hk-server-netstat: rx_frames=",
            ],
        }
    }

    fn exit_code(self) -> i32 {
        match self {
            Self::Boot
            | Self::Timer
            | Self::Framebuffer
            | Self::Shell
            | Self::Input
            | Self::Preempt
            | Self::Restart
            | Self::Ext4Root
            | Self::HolyFs
            | Self::Net => QEMU_SUCCESS,
            Self::Smp => QEMU_SUCCESS,
            Self::Ud2 | Self::Page => QEMU_EXCEPTION,
        }
    }

    fn log_name(self) -> &'static str {
        match self {
            Self::Boot => "build/serial.log",
            Self::Ud2 => "build/serial-fault.log",
            Self::Page => "build/serial-page-fault.log",
            Self::Timer => "build/serial-timer.log",
            Self::Framebuffer => "build/serial-fb.log",
            Self::Shell => "build/serial-shell.log",
            Self::Input => "build/serial-input.log",
            Self::Preempt => "build/serial-preempt.log",
            Self::Smp => "build/serial-smp.log",
            Self::Restart => "build/serial-restart.log",
            Self::Ext4Root => "build/serial-ext4root.log",
            Self::HolyFs => "build/serial-holyfs.log",
            Self::Net => "build/serial-net.log",
        }
    }

    /// 需要断言先后的输出片段。
    fn ordered(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Preempt => Some(("hk-server-demo 收到回复 42", "hk-server-spin: done")),
            _ => None,
        }
    }
}

/// 启动内核并校验串口输出。
pub fn run(selection: Selection, release: bool, mode: SmokeMode) -> Result<()> {
    let targets = selection.targets();
    if targets.len() != 1 || targets[0] != Target::X86_64Qemu {
        return Err(Failure::new("smoke 只支持 x86_64-qemu"));
    }

    let iso = crate::image::run(selection, release, mode.cmdline())?;
    let disk = prepare_disk(mode)?;
    let log = workspace::root().join(mode.log_name());
    let _ = fs::remove_file(&log);

    let socket = workspace::root().join("build/qmp.sock");
    if mode == SmokeMode::Input {
        let _ = fs::remove_file(&socket);
    }

    let echo = if mode == SmokeMode::Net {
        let listener = TcpListener::bind(("0.0.0.0", NET_TCP_PORT))
            .map_err(|error| Failure::new(format!("绑定 TCP echo 失败：{error}")))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| Failure::new(format!("设置 TCP echo 非阻塞失败：{error}")))?;
        Some(listener)
    } else {
        None
    };

    let mut command = Command::new("qemu-system-x86_64");
    command
        .args(["-M", "q35", "-m", "256M", "-cdrom"])
        .arg(&iso)
        .args(["-boot", "d", "-display", "none", "-no-reboot", "-serial"])
        .arg(format!("file:{}", log.display()))
        .arg("-drive")
        .arg(format!(
            "file={},if=none,id=disk,format=raw",
            disk.display()
        ))
        .args(["-device", "virtio-blk-pci,drive=disk"])
        .args(["-device", "isa-debug-exit,iobase=0xf4,iosize=0x04"]);
    if mode == SmokeMode::Smp {
        command.args(["-smp", "4"]);
    }
    if mode == SmokeMode::Net {
        command
            .args(["-netdev", "user,id=net0"])
            .args(["-device", "virtio-net-pci,netdev=net0"]);
    }
    if mode == SmokeMode::Input {
        command
            .arg("-qmp")
            .arg(format!("unix:{},server=on,wait=off", socket.display()));
    }

    let mut child = command
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| Failure::new(format!("启动 qemu-system-x86_64 失败：{error}")))?;

    if mode == SmokeMode::Input {
        thread::sleep(Duration::from_secs(3));
        inject(&socket, "echo hello-ioapic\n")?;
        thread::sleep(Duration::from_millis(500));
        inject(&socket, "use boot.hsh\n")?;
        thread::sleep(Duration::from_millis(500));
        inject(&socket, "exit\n")?;
    }
    if let Some(listener) = echo {
        serve_tcp_echo(listener)?;
    }

    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() > deadline => {
                let _ = child.kill();
                break None;
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(error) => return Err(Failure::new(format!("等待 QEMU 失败：{error}"))),
        }
    };

    let expected = mode.expected();
    let read_deadline = Instant::now() + Duration::from_secs(1);
    let text = loop {
        let bytes = fs::read(&log).unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        if expected.iter().all(|item| text.contains(item)) || Instant::now() > read_deadline {
            break text;
        }
        thread::sleep(Duration::from_millis(20));
    };
    let expected_code = mode.exit_code();
    match status {
        Some(status) if status.code() == Some(expected_code) => {}
        Some(status) => {
            return Err(Failure::new(format!(
                "QEMU 退出码 {:?}，期望 {expected_code}\n串口输出：\n{text}",
                status.code()
            )));
        }
        None => {
            return Err(Failure::new(format!("QEMU 超时未退出\n串口输出：\n{text}")));
        }
    }

    for expected in mode.expected() {
        if !text.contains(expected) {
            return Err(Failure::new(format!(
                "串口输出缺少「{expected}」\n串口输出：\n{text}"
            )));
        }
    }

    if let Some((first, second)) = mode.ordered() {
        let in_order = matches!(
            (text.find(first), text.find(second)),
            (Some(first_at), Some(second_at)) if first_at < second_at
        );
        if !in_order {
            return Err(Failure::new(format!(
                "串口输出顺序不符：「{first}」应在「{second}」之前\n串口输出：\n{text}"
            )));
        }
    }

    if mode == SmokeMode::HolyFs {
        verify_holyfs(&disk)?;
    } else if matches!(
        mode,
        SmokeMode::Boot | SmokeMode::Restart | SmokeMode::Ext4Root
    ) {
        verify_ext4(&disk)?;
    }
    Ok(())
}

/// 宿主墙钟，用于给测试镜像写入正常时间戳。
struct HostClock;

impl hk_fs_ext4::WallClock for HostClock {
    fn now(&self) -> Option<CivilTime> {
        host_civil_time()
    }
}

/// std::time 的原点；宿主工具只在镜像生成时使用。
const HOST_TIME_ORIGIN: CivilTime = CivilTime {
    year: 1970,
    month: 1,
    day: 1,
    hour: 0,
    minute: 0,
    second: 0,
    nanosecond: 0,
};

fn host_civil_time() -> Option<CivilTime> {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    Some(HOST_TIME_ORIGIN.add_seconds(elapsed.as_secs(), elapsed.subsec_nanos()))
}

/// 接受一次 QEMU 宿主的 TCP 连接并回显。
fn serve_tcp_echo(listener: TcpListener) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .map_err(|error| Failure::new(format!("设置 TCP 读取超时失败：{error}")))?;
                let mut buffer = [0_u8; 16];
                let length = stream
                    .read(&mut buffer)
                    .map_err(|error| Failure::new(format!("TCP echo 读取失败：{error}")))?;
                stream
                    .write_all(&buffer[..length])
                    .map_err(|error| Failure::new(format!("TCP echo 写回失败：{error}")))?;
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() > deadline {
                    return Err(Failure::new("QEMU TCP 未连接到宿主 echo"));
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => {
                return Err(Failure::new(format!("TCP echo accept 失败：{error}")));
            }
        }
    }
}

/// 生成新的测试磁盘。
fn prepare_disk(mode: SmokeMode) -> Result<PathBuf> {
    let path = workspace::root().join("build/test-disk.img");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| Failure::new(format!("创建 {} 失败：{error}", parent.display())))?;
    }
    let blocks = TEST_DISK_BYTES / SECTOR_SIZE as u64;
    let disk = MemoryDisk::new(blocks, SECTOR_SIZE);
    let disk = if mode == SmokeMode::HolyFs {
        hk_fs_holyfs::format(disk)
            .map_err(|error| Failure::new(format!("格式化 HolyFS 测试磁盘失败：{error}")))?
    } else {
        hk_fs_ext4::format_with_wall_clock(disk, HostClock)
            .map_err(|error| Failure::new(format!("格式化 ext4 测试磁盘失败：{error}")))?
    };
    fs::write(&path, disk.into_data())
        .map_err(|error| Failure::new(format!("写入 {} 失败：{error}", path.display())))?;
    Ok(path)
}

/// 从磁盘镜像重新挂载 HolyFS，确认标记文件已经持久化。
fn verify_holyfs(path: &Path) -> Result<()> {
    let bytes = fs::read(path)
        .map_err(|error| Failure::new(format!("读取 {} 失败：{error}", path.display())))?;
    let disk = MemoryDisk::from_data(bytes, SECTOR_SIZE)
        .ok_or_else(|| Failure::new("测试磁盘长度不是扇区整数倍"))?;
    let backend = hk_fs_holyfs::HolyFs::mount(disk)
        .map_err(|error| Failure::new(format!("重新挂载 HolyFS 失败：{error}")))?;
    let root = backend.root();
    let node = backend
        .lookup(root, "hello")
        .map_err(|error| Failure::new(format!("HolyFS 标记文件缺失：{error:?}")))?;
    let mut buffer = [0_u8; 32];
    let read = backend
        .read(node, 0, &mut buffer)
        .map_err(|error| Failure::new(format!("读取 HolyFS 标记文件失败：{error:?}")))?;
    if &buffer[..read] != FS_MARKER {
        return Err(Failure::new(format!(
            "HolyFS 标记内容不符：{:?}",
            &buffer[..read]
        )));
    }
    Ok(())
}

/// 从磁盘镜像重新挂载 ext4，确认 VFS 写入的标记文件已经持久化。
fn verify_ext4(path: &Path) -> Result<()> {
    let bytes = fs::read(path)
        .map_err(|error| Failure::new(format!("读取 {} 失败：{error}", path.display())))?;
    let disk = MemoryDisk::from_data(bytes, SECTOR_SIZE)
        .ok_or_else(|| Failure::new("测试磁盘长度不是扇区整数倍"))?;
    let backend = hk_fs_ext4::Ext4Backend::mount(disk)
        .map_err(|error| Failure::new(format!("重新挂载 ext4 失败：{error}")))?;
    let root = backend.root();
    let node = backend
        .lookup(root, "hello")
        .or_else(|_| {
            backend
                .lookup(root, "disk")
                .and_then(|disk| backend.lookup(disk, "hello"))
        })
        .map_err(|error| Failure::new(format!("ext4 标记文件缺失：{error:?}")))?;
    let mut buffer = [0_u8; 32];
    let read = backend
        .read(node, 0, &mut buffer)
        .map_err(|error| Failure::new(format!("读取 ext4 标记文件失败：{error:?}")))?;
    if &buffer[..read] != FS_MARKER {
        return Err(Failure::new(format!(
            "ext4 标记内容不符：{:?}",
            &buffer[..read]
        )));
    }
    Ok(())
}
