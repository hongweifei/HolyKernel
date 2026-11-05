//! QEMU 冒烟测试：启动内核并检查串口输出。

mod qmp;

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use hk_block::{MemoryDisk, SECTOR_SIZE};
use hk_vfs::PathBackend;
use qmp::{inject, inject_key, inject_pointer, inject_pointer_drag};

use crate::disk::HostClock;
use crate::failure::{Failure, Result};
use crate::target::{Selection, Target};
use crate::workspace;

/// 等待内核退出的上限。
const TIMEOUT: Duration = Duration::from_secs(30);

/// ext4 自检写入的标记内容。
const FS_MARKER: &[u8] = b"HolyKernel ext4 ok\n";

/// QEMU 用户网络中的宿主 TCP echo 端口。
const NET_TCP_PORT: u16 = 45678;

/// QEMU 用户网络中的宿主 UDP echo 端口。
const NET_UDP_PORT: u16 = 45679;

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
    /// 原生静态网络配置（关闭 DHCP）
    #[value(name = "netstatic")]
    NetStatic,
    /// virtio-net 驱动退出后重启并恢复网络
    #[value(name = "netrestart")]
    NetRestart,
    /// 共享 Frame 收发环（零拷贝）
    #[value(name = "netzerocopy")]
    NetZerocopy,
    /// 原生 Socket Handle 客户端自检
    #[value(name = "netsocket")]
    NetSocket,
    /// Console 服务与内核输入自检
    #[value(name = "console")]
    Console,
    /// Display 服务帧缓冲映射与测试图案
    #[value(name = "display")]
    Display,
    /// Console 服务的图形终端窗口
    #[value(name = "terminal")]
    Terminal,
    /// 用户态 virtio-input 驱动
    #[value(name = "inputdriver")]
    InputDriver,
    /// virtio-tablet 指针与点击焦点
    #[value(name = "pointer")]
    Pointer,
    /// HolyDoc 文档窗口
    #[value(name = "holydoc")]
    Holydoc,
    /// Debug 内核统计窗口
    #[value(name = "debug")]
    Debug,
    /// Debug 异常与符号化回溯
    #[value(name = "debugfault")]
    DebugFault,
    /// 调试视图客户端
    #[value(name = "dbg")]
    Dbg,
    /// Display 热重启与 Console 重连
    #[value(name = "displayrestart")]
    DisplayRestart,
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
                "shell-test help;focus hk-server-echo;tasks;start hk-server-device;tasks;handles;services;restart hk-server-demo;services;start hk-server-fault;start hk-server-fork;start hk-server-latency;tasks;trace;stats",
            ),
            Self::Input => Some("shell-interactive"),
            Self::Preempt => None,
            Self::Smp => Some("shell-test tasks;stats"),
            Self::Restart => Some("restart-test"),
            Self::Ext4Root => Some("vfs.root=ext4"),
            Self::HolyFs => Some("vfs.fs=holyfs"),
            Self::Net => Some("net-test net.dns=localhost shell-test net"),
            Self::NetStatic => Some(
                "net-test net.dhcp=off net.ip=10.0.2.15 net.prefix=24 net.gw=10.0.2.2 net.resolver=10.0.2.3 net.dns=localhost shell-test net",
            ),
            Self::NetRestart => Some("net-test net.restart net.dns=localhost shell-test net"),
            Self::NetZerocopy => Some("net-test net.zerocopy net.dns=localhost shell-test net"),
            Self::NetSocket => Some("net-test net.no-selftest net.socket-test"),
            Self::Console => Some("console-test"),
            Self::Display => Some("theme=light ui.scale=2 display-test"),
            Self::Terminal => Some("terminal-test"),
            Self::InputDriver => Some("input-test input-driver terminal-test"),
            Self::Pointer => Some("pointer-test input-driver"),
            Self::Holydoc => Some("vfs.fs=holyfs holydoc-test"),
            Self::Debug => Some("debug-test"),
            Self::DebugFault => Some("debug-crash"),
            Self::Dbg => Some("dbg-test"),
            Self::DisplayRestart => Some("display-restart-test"),
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
                "hk-server-holyfs: mounted H:",
                "hk-server-holyfs: drive H:",
                "hk-server-demo fs ok",
                "fs 1 MiB 读：",
            ],
            Self::Net => &[
                "hk-server-virtio-net: mac=",
                "hk-server-net: ready",
                "hk-server-net: ping ok",
                "hk-server-net: tcp ok",
                "hk-server-net: udp ok",
                "hk-server-net: dns ok",
                "127.0.0.1",
                "hk-server-netstat: mac=",
                "hk-server-netstat: rx_frames=",
                "hk-server-netstat: route=",
                "hk-server-netstat: arp",
                "hk-server-netstat: config dhcp=",
            ],
            Self::NetStatic => &[
                "hk-server-virtio-net: mac=",
                "hk-server-net: static ok",
                "hk-server-net: ping ok",
                "hk-server-net: tcp ok",
                "hk-server-net: udp ok",
                "hk-server-net: dns ok",
                "127.0.0.1",
                "hk-server-netstat: config dhcp=off",
                "hk-server-netstat: config dns=10.0.2.3",
                "hk-server-netstat: route=",
                "hk-server-netstat: arp",
            ],
            Self::NetRestart => &[
                "hk-server-virtio-net: mac=",
                "hk-server-net: ready",
                "hk-server-net: driver restart",
                "hk-server-net: restart ok",
                "hk-server-net: tcp ok",
                "hk-server-net: udp ok",
                "hk-server-net: dns ok",
                "127.0.0.1",
                "hk-server-netstat: config dhcp=",
            ],
            Self::NetZerocopy => &[
                "hk-server-virtio-net: shared frames ready",
                "hk-server-net: shared frames ready",
                "hk-server-net: ready",
                "hk-server-net: ping ok",
                "hk-server-net: tcp ok",
                "hk-server-net: udp ok",
                "hk-server-net: dns ok",
                "127.0.0.1",
                "hk-server-netstat: config dhcp=",
            ],
            Self::NetSocket => &[
                "hk-server-net: ready",
                "hk-server-netsock: tcp ok",
                "hk-server-netsock: udp ok",
                "hk-server-netsock: dns ok",
            ],
            Self::Console => &["hk-server-console-test: got hello"],
            Self::Display => &[
                "hk-server-display: theme light",
                "hk-server-display: scale 2",
                "hk-server-display: ready",
                "hk-server-display: title ok",
                "hk-server-display: composite ok",
                "hk-server-display: focus 1",
                "hk-server-display-test: ready",
                "hk-server-display-test: key 97 win 1",
                "hk-server-display-test: done",
            ],
            Self::Terminal => &[
                "hk-server-console: window",
                "hk-server-console: utf8 ok",
                "hk-server-console: cjk ok",
                "hk-server-display: composite ok",
                "hk-server-terminal-test: line hi",
                "hk-server-terminal-test: done",
            ],
            Self::InputDriver => &[
                "hk-server-input: ready",
                "hk-server-display: composite ok",
                "hk-server-terminal-test: line hi",
                "hk-server-terminal-test: done",
            ],
            Self::Pointer => &[
                "hk-server-input: ready",
                "hk-server-display: drag 1",
                "hk-server-display: taskbar 1",
                "hk-server-display: resize 1 464x348",
                "hk-server-display: maximize 1 1280x748",
                "hk-server-display: restore 1 464x348",
                "hk-server-display: minimize 1",
                "hk-server-display: pointer click 1",
                "hk-server-pointer-test: clicked",
                "hk-server-display: close 1",
                "hk-server-pointer-test: closed 1",
            ],
            Self::Holydoc => &[
                "hk-server-display: composite ok",
                "hk-server-holyfs: drive H:",
                "hk-server-holydoc: document H:/Welcome",
                "hk-server-holydoc: document H:/System/Overview",
                "hk-server-holydoc: ready 1",
                "hk-server-holydoc: doc H:/Welcome",
                "hk-server-holydoc: drive docs 5",
                "hk-server-holydoc: exec version",
                "hk-server-holydoc: snapshot ok",
                "hk-server-holydoc: token ok",
                "hk-server-holydoc: tag ok",
                "hk-server-holydoc: click H:/System",
                "hk-server-holydoc: open H:/System",
                "hk-server-holydoc: open H:/System/Overview",
                "hk-server-holydoc: scroll ",
                "hk-server-holydoc: back H:/System",
                "hk-server-holydoc: done",
            ],
            Self::Debug => &[
                "hk-server-display: composite ok",
                "hk-server-debug: ready 1",
                "hk-server-debug: threads ",
                "hk-server-debug: objects ",
                "hk-server-debug: symbol 0xffffffff80000000 [kernel] _start",
                "hk-server-debug: done",
            ],
            Self::DebugFault => &[
                "hk-server-display: composite ok",
                "hk-server-debug: ready 1",
                "hk-server-debug: fault symbolized",
                "hk-server-debug: done",
            ],
            Self::Dbg => &[
                "hk-server-dbg: ready",
                "hk-server-dbg: tasks",
                "hk-server-dbg: done",
            ],
            Self::DisplayRestart => &[
                "hk-server-console: window 1",
                "hk-server-display: restart requested",
                "hk-server-console: reconnected",
                "hk-server-display-restart-test: restarted",
                "hk-server-console: done",
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
            | Self::Net
            | Self::NetStatic
            | Self::NetRestart
            | Self::NetZerocopy
            | Self::NetSocket
            | Self::Console
            | Self::Display
            | Self::Terminal
            | Self::InputDriver
            | Self::Pointer
            | Self::Holydoc
            | Self::Debug
            | Self::DebugFault
            | Self::Dbg
            | Self::DisplayRestart => QEMU_SUCCESS,
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
            Self::NetStatic => "build/serial-netstatic.log",
            Self::NetRestart => "build/serial-netrestart.log",
            Self::NetZerocopy => "build/serial-netzerocopy.log",
            Self::NetSocket => "build/serial-netsocket.log",
            Self::Console => "build/serial-console.log",
            Self::Display => "build/serial-display.log",
            Self::Terminal => "build/serial-terminal.log",
            Self::InputDriver => "build/serial-inputdriver.log",
            Self::Pointer => "build/serial-pointer.log",
            Self::Holydoc => "build/serial-holydoc.log",
            Self::Debug => "build/serial-debug.log",
            Self::DebugFault => "build/serial-debugfault.log",
            Self::Dbg => "build/serial-dbg.log",
            Self::DisplayRestart => "build/serial-displayrestart.log",
        }
    }

    /// 是否为网络模式（需要宿主 echo 与 QEMU 网卡）。
    fn is_network(self) -> bool {
        matches!(
            self,
            Self::Net | Self::NetStatic | Self::NetRestart | Self::NetZerocopy | Self::NetSocket
        )
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

    let echo = if mode.is_network() {
        let listener = TcpListener::bind(("0.0.0.0", NET_TCP_PORT))
            .map_err(|error| Failure::new(format!("绑定 TCP echo 失败：{error}")))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| Failure::new(format!("设置 TCP echo 非阻塞失败：{error}")))?;
        Some(listener)
    } else {
        None
    };
    let udp = if mode.is_network() {
        let socket = UdpSocket::bind(("0.0.0.0", NET_UDP_PORT))
            .map_err(|error| Failure::new(format!("绑定 UDP echo 失败：{error}")))?;
        socket
            .set_read_timeout(Some(Duration::from_millis(100)))
            .map_err(|error| Failure::new(format!("设置 UDP echo 超时失败：{error}")))?;
        Some(socket)
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
    if mode.is_network() {
        command
            .args(["-netdev", "user,id=net0"])
            .args(["-device", "virtio-net-pci,netdev=net0"]);
    }
    if mode == SmokeMode::InputDriver {
        command.args(["-device", "virtio-keyboard-pci"]);
    }
    if mode == SmokeMode::Pointer {
        command.args(["-device", "virtio-tablet-pci"]);
    }
    if matches!(
        mode,
        SmokeMode::Input
            | SmokeMode::Console
            | SmokeMode::Display
            | SmokeMode::Terminal
            | SmokeMode::InputDriver
            | SmokeMode::Pointer
    ) {
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
    if mode == SmokeMode::Console {
        thread::sleep(Duration::from_secs(5));
        inject(&socket, "hello\n")?;
    }
    if mode == SmokeMode::Display {
        thread::sleep(Duration::from_secs(5));
        inject_key(&socket, "tab")?;
        thread::sleep(Duration::from_millis(300));
        inject(&socket, "a")?;
    }
    if mode == SmokeMode::Terminal {
        thread::sleep(Duration::from_secs(5));
        inject(&socket, "hi\n")?;
    }
    if mode == SmokeMode::InputDriver {
        thread::sleep(Duration::from_secs(5));
        inject(&socket, "hi\n")?;
    }
    if mode == SmokeMode::Pointer {
        thread::sleep(Duration::from_secs(5));
        // 任务栏占 28px：1280x800 下窗口位于 440,224（400x300），标题栏中心约 640,236。
        inject_pointer_drag(&socket, 0x3FFF, 0x25C2, 0x4666, 0x25C2)?;
        thread::sleep(Duration::from_millis(300));
        // 任务栏第一个按钮约在 (88,786)。
        inject_pointer(&socket, 0x08CC, 0x7DC1)?;
        thread::sleep(Duration::from_millis(300));
        // 窗口右下角缩放把手约在 (899,542)，拖到 (963,590)。
        inject_pointer_drag(&socket, 0x59E5, 0x56B6, 0x604C, 0x5E65)?;
        thread::sleep(Duration::from_millis(300));
        // 最大化按钮约在 (944,236)，还原后再点最小化 (930,236)。
        inject_pointer(&socket, 0x5E66, 0x25C2)?;
        thread::sleep(Duration::from_millis(300));
        inject_pointer(&socket, 0x7D7A, 0x01EB)?;
        thread::sleep(Duration::from_millis(300));
        inject_pointer(&socket, 0x5CFF, 0x25C2)?;
        thread::sleep(Duration::from_millis(300));
        // 点击任务栏按钮恢复最小化窗口。
        inject_pointer(&socket, 0x08CC, 0x7DC1)?;
        thread::sleep(Duration::from_millis(300));
        inject_pointer(&socket, 0x3FFF, 0x3FFF)?;
        thread::sleep(Duration::from_millis(300));
        // 还原后窗口右上方约在 (958,236) 的关闭按钮。
        inject_pointer(&socket, 0x5FCC, 0x25C2)?;
    }
    let udp_echo = udp.map(|socket| thread::spawn(move || serve_udp_echo(socket)));
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

    if let Some(handle) = udp_echo {
        let _ = handle.join();
    }

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

/// 回显一次 UDP 数据报，供 QEMU 内 UDP 自检使用。
fn serve_udp_echo(socket: UdpSocket) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut buffer = [0_u8; 64];
    loop {
        match socket.recv_from(&mut buffer) {
            Ok((length, peer)) => {
                socket
                    .send_to(&buffer[..length], peer)
                    .map_err(|error| Failure::new(format!("UDP echo 写回失败：{error}")))?;
                return Ok(());
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if Instant::now() > deadline {
                    return Err(Failure::new("QEMU UDP 未发送到宿主 echo"));
                }
            }
            Err(error) => {
                return Err(Failure::new(format!("UDP echo 接收失败：{error}")));
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
    let blocks = crate::disk::DISK_BYTES / SECTOR_SIZE as u64;
    let disk = MemoryDisk::new(blocks, SECTOR_SIZE);
    let disk = if matches!(mode, SmokeMode::HolyFs | SmokeMode::Holydoc) {
        let disk = hk_fs_holyfs::format(disk)
            .map_err(|error| Failure::new(format!("格式化 HolyFS 测试磁盘失败：{error}")))?;
        crate::disk::seed_holy_docs(disk)?
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
    let volume = hk_fs_holyfs::HolyFs::mount(disk)
        .map_err(|error| Failure::new(format!("重新挂载 HolyFS 失败：{error}")))?;
    let document = volume
        .resolve("/hello")
        .map_err(|error| Failure::new(format!("HolyFS 标记文件缺失：{error:?}")))?;
    let content = volume
        .read_document_id(document)
        .map_err(|error| Failure::new(format!("读取 HolyFS 标记文件失败：{error:?}")))?;
    if content != FS_MARKER {
        return Err(Failure::new(format!("HolyFS 标记内容不符：{:?}", content)));
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
