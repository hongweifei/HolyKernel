//! QEMU 冒烟测试：启动内核并检查串口输出。

mod qmp;

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use hk_block::{BlockDevice, DiskSlice, MemoryDisk, SECTOR_SIZE};
use hk_vfs::PathBackend;
use qmp::{inject, inject_key, inject_pointer, inject_pointer_drag};

use crate::disk::HostClock;
use crate::failure::{Failure, Result};
use crate::target::{Selection, Target};
use crate::workspace;

/// 等待内核退出的上限。
const TIMEOUT: Duration = Duration::from_secs(30);
/// Shell 冒烟覆盖多服务重启、热替换与文件系统检查，调试构建需要更长预算。
const SHELL_TIMEOUT: Duration = Duration::from_secs(90);

/// HolyDoc 自检含快照与多次渲染，debug 构建需要更长时间。
const HOLYDOC_TIMEOUT: Duration = Duration::from_secs(90);

/// ext4 自检写入的标记内容。
const FS_MARKER: &[u8] = b"HolyKernel ext4 ok\n";

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
    /// Tempo 策略服务准入与自助附加
    #[value(name = "tempo")]
    Tempo,
    /// sleep 到期到恢复运行的唤醒延迟
    #[value(name = "wakeup")]
    Wakeup,
    /// Realtime EDF 硬截止期与预算节流
    #[value(name = "realtime")]
    Realtime,
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
    /// virtio-rng 熵源驱动
    #[value(name = "rng")]
    Rng,
    /// virtio-console 驱动与宿主回显
    #[value(name = "vcon")]
    Vcon,
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
    /// HolyDoc 窗口最大化缩放与重排
    #[value(name = "holydocresize")]
    HolyDocResize,
    /// Workbench 文档浏览与编辑
    #[value(name = "workbench")]
    Workbench,
    /// Debug 内核统计窗口
    #[value(name = "debug")]
    Debug,
    /// 任务管理器进程与性能窗口
    #[value(name = "taskman")]
    Taskman,
    /// Debug 异常与符号化回溯
    #[value(name = "debugfault")]
    DebugFault,
    /// 调试视图客户端
    #[value(name = "dbg")]
    Dbg,
    /// Display 热重启与 Console 重连
    #[value(name = "displayrestart")]
    DisplayRestart,
    /// 运行中应用热替换
    #[value(name = "hotswap")]
    Hotswap,
    /// Native Task 检查点与恢复
    #[value(name = "checkpoint")]
    Checkpoint,
    /// Checkpoint Server 服务与客户端
    #[value(name = "checkpointsrv")]
    CheckpointServer,
    /// HMP 内存对象面 conformance
    #[value(name = "memory")]
    Memory,
}

impl SmokeMode {
    fn cmdline(self, net: Option<(u16, u16)>) -> Option<String> {
        match self {
            Self::Boot => None,
            Self::Ud2 => Some(String::from("fault-test")),
            Self::Page => Some(String::from("page-fault-test")),
            Self::Timer => Some(String::from("timer-test")),
            Self::Framebuffer => Some(String::from("fb-test")),
            Self::Shell => Some(String::from(
                "shell-test help;focus hk-server-echo;tasks;start hk-server-device;tasks;handles;services;restart hk-server-demo;services;start hk-server-fault;start hk-server-fork;start hk-server-latency;replace hk-server-echo hk-server-spin;tasks;trace;stats",
            )),
            Self::Input => Some(String::from("shell-interactive")),
            Self::Preempt => None,
            Self::Tempo => Some(String::from("tempo-test")),
            Self::Wakeup => Some(String::from("wakeup-test")),
            Self::Realtime => Some(String::from("realtime-test")),
            Self::Smp => Some(String::from("shell-test tasks;stats")),
            Self::Restart => Some(String::from("restart-test")),
            Self::Ext4Root => Some(String::from("vfs.root=ext4")),
            Self::HolyFs => Some(String::from("vfs.fs=holyfs")),
            Self::Net => Some(format!(
                "net-test net.dns=localhost shell-test net net.tcp={} net.udp={}",
                net.unwrap().0,
                net.unwrap().1
            )),
            Self::NetStatic => Some(format!(
                "net-test net.dhcp=off net.ip=10.0.2.15 net.prefix=24 net.gw=10.0.2.2 net.resolver=10.0.2.3 net.dns=localhost shell-test net net.tcp={} net.udp={}",
                net.unwrap().0,
                net.unwrap().1
            )),
            Self::NetRestart => Some(format!(
                "net-test net.restart net.dns=localhost shell-test net net.tcp={} net.udp={}",
                net.unwrap().0,
                net.unwrap().1
            )),
            Self::NetZerocopy => Some(format!(
                "net-test net.zerocopy net.dns=localhost shell-test net net.tcp={} net.udp={}",
                net.unwrap().0,
                net.unwrap().1
            )),
            Self::NetSocket => Some(format!(
                "net-test net.no-selftest net.socket-test net.tcp={} net.udp={}",
                net.unwrap().0,
                net.unwrap().1
            )),
            Self::Console => Some(String::from("console-test")),
            Self::Rng => Some(String::from("rng-test")),
            Self::Vcon => Some(String::from("vcon-test")),
            Self::Display => Some(String::from("theme=light ui.scale=2 display-test")),
            Self::Terminal => Some(String::from("terminal-test")),
            Self::InputDriver => Some(String::from("input-test input-driver terminal-test")),
            Self::Pointer => Some(String::from("pointer-test input-driver")),
            Self::Holydoc => Some(String::from("vfs.fs=holyfs holydoc-test")),
            Self::HolyDocResize => Some(String::from(
                "vfs.fs=holyfs holydoc-resize-test input-driver",
            )),
            Self::Workbench => Some(String::from("vfs.fs=holyfs workbench-test")),
            Self::Debug => Some(String::from("debug-test")),
            Self::Taskman => Some(String::from("taskman-test")),
            Self::DebugFault => Some(String::from("debug-crash")),
            Self::Dbg => Some(String::from("dbg-test")),
            Self::DisplayRestart => Some(String::from("display-restart-test")),
            Self::Hotswap => Some(String::from(
                "hotswap-test shell-test help;replace hk-server-echo hk-server-fault;tasks",
            )),
            Self::Checkpoint => Some(String::from(
                "checkpoint-test shell-test help;checkpoint hk-server-memory;restore-checkpoint hk-server-memory;tasks",
            )),
            Self::CheckpointServer => Some(String::from("vfs.fs=holyfs checkpoint-srv-test")),
            Self::Memory => Some(String::from("memory-test")),
        }
    }

    fn expected(self) -> &'static [&'static str] {
        match self {
            Self::Boot => &[
                "TSC 频率",
                "启动信息校验通过",
                "System Capacity ",
                "System Arena ",
                "内核初始化耗时",
                "加载服务并进入用户态",
                "hk-server-demo running",
                "hk-server-virtio: block irq bound",
                "hk-server-echo 收到请求 21",
                "hk-server-demo 收到回复 42",
                "hk-server-demo arena ok",
                "hk-server-demo sync ok",
                "hk-server-demo worker ok",
                "hk-server-demo hmp ok",
                "hk-server-memory: capacity ok",
                "hk-server-memory: alloc",
                "hk-server-memory: fault",
                "hk-server-echo 收到句柄",
                "hk-server-demo 句柄转移 ok",
                "hk-server-echo 已发出通知",
                "hk-server-demo 通知等待 ok",
                "hk-server-device: 发现",
                "hk-server-virtio: virtio-blk sectors=131072",
                "hk-server-ext4: mounted E:",
                "hk-server-path: rtc ok",
                "hk-server-path: ext4 mounted",
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
                "任务表（",
                "父",
                "句柄",
                "Arena",
                "页表根",
                "/2048K",
                "Handle 计数",
                "重启",
                "已重启：hk-server-demo",
                "hk-server-demo running",
                "hk-server-demo 收到回复 42",
                "hk-server-demo 句柄转移 ok",
                "hk-server-demo 通知等待 ok",
                "用户异常：页错误",
                "hk-server-fault",
                "已替换：hk-server-echo",
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
            Self::Tempo => &[
                "hk-server-tempo: ready",
                "hk-server-tempo-test: ok weight=512",
            ],
            Self::Wakeup => &["hk-server-wakeup: min=", "hk-server-wakeup: ok"],
            Self::Realtime => &["hk-server-realtime: overruns=0", "hk-server-realtime: ok"],
            Self::Smp => &[
                "本地 APIC：4 个",
                "AP 在线：3 个（启动 3，IPI 1 次）",
                "AP 空闲 IPI 唤醒完成，TLB shootdown",
                "hk-server-demo sync ok",
                "hk-server-demo worker ok",
                "CPU 调度：",
                "CPU1=",
            ],
            Self::Restart => &[
                "hk-server-ext4: mounted E:",
                "hk-server-path: rtc ok",
                "hk-server-path: ext4 mounted",
                "hk-server-demo fs ok",
                "hk-server-demo 驱动重启 ok",
                "fs 重连 ok",
                "fs 1 MiB 读：",
            ],
            Self::Ext4Root => &[
                "hk-server-ext4: mounted E:",
                "hk-server-path: ext4 mounted",
                "hk-server-demo fs ok",
                "fs 1 MiB 读：",
            ],
            Self::HolyFs => &[
                "hk-server-virtio: block irq bound",
                "hk-server-holyfs: async bulk ok",
                "hk-server-holyfs: mounted H:",
                "hk-server-holyfs: drive H:",
                "hk-server-holyfs: mounted I:",
                "hk-server-holyfs: drive I:",
                "hk-server-path: drives via hk-server-vfs",
                "hk-server-demo fs ok",
                "fs 1 MiB 读：",
            ],
            Self::Net => &[
                "hk-server-virtio-net: mac=",
                "hk-server-virtio-net: irq intx",
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
            Self::Rng => &[
                "hk-server-virtio: entropy ok",
                "hk-server-virtio: rng ready",
            ],
            Self::Vcon => &[
                "hk-server-virtio: console ready",
                "hk-server-virtio: console echo ok",
            ],
            Self::Display => &[
                "hk-server-display: font scalable 32",
                "hk-server-display: theme light",
                "hk-server-display: scale 2",
                "hk-server-display: ready",
                "hk-server-display: title ok",
                "hk-server-display: composite ok",
                "hk-server-display: surface ",
                "hk-server-display: clipboard set",
                "hk-server-display: clipboard get",
                "hk-server-display: focus 1",
                "hk-server-display-test: clipboard holy",
                "hk-server-display-test: ready",
                "hk-server-display-test: key 97 win 1",
                "hk-server-display-test: done",
                "hk-server-display: scanout 1 on",
                "hk-server-display: scanout 2 on",
            ],
            Self::Terminal => &[
                "hk-server-console: window",
                "hk-server-console: utf8 ok",
                "hk-server-console: cjk ok",
                "hk-server-console: sgr ok",
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
                "hk-server-display: cursor hint 1 hand",
                "hk-server-display: cursor move",
                "hk-server-display: cursor hand",
                "hk-server-display: cursor resize-nwse",
                "hk-server-display: drag 1",
                "hk-server-display: taskbar 1",
                "hk-server-display: resize 1 464x348",
                "hk-server-display: surface resize 1 464x348",
                "hk-server-pointer-test: resize 464x348",
                "hk-server-display: maximize 1 1280x748",
                "hk-server-display: restore 1 464x348",
                "hk-server-display: minimize 1",
                "hk-server-display: scanout 1 off",
                "hk-server-pointer-test: pointer 1",
                "hk-server-display: pointer click 1",
                "hk-server-pointer-test: clicked",
                "hk-server-display: close 1",
                "hk-server-pointer-test: closed 1",
            ],
            Self::Holydoc => &[
                "hk-server-display: composite ok",
                "hk-server-holyfs: drive H:",
                "hk-server-holyfs: drive I:",
                "hk-server-holydoc: drives ok",
                "hk-server-holydoc: document H:/Welcome",
                "hk-server-holydoc: document H:/System/Overview",
                "hk-server-holydoc: render worker ok",
                "hk-server-holydoc: ready 1",
                "hk-server-holydoc: doc H:/Welcome",
                "hk-server-holydoc: drive docs ",
                "hk-server-holydoc: exec version",
                "hk-server-holydoc: snapshot ok",
                "hk-server-holydoc: token ok",
                "hk-server-holydoc: tag ok",
                "hk-server-holydoc: maintenance ok",
                "hk-server-holydoc: retention ok",
                "hk-server-holydoc: quota ok",
                "hk-server-holydoc: stream ok",
                "hk-server-holydoc: click H:/System",
                "hk-server-holydoc: open H:/System",
                "hk-server-holydoc: open H:/System/Overview",
                "hk-server-holydoc: scroll ",
                "hk-server-holydoc: back H:/System",
                "hk-server-holydoc: done",
            ],
            Self::HolyDocResize => &[
                "hk-server-display: composite ok",
                "hk-server-holydoc: ready 1",
                "hk-server-display: maximize 1 1280x748",
                "hk-server-holydoc: resize 1280x748",
                "hk-server-display: surface resize 1 1280x748",
                "hk-server-holydoc: resize pixel ok",
                "hk-server-holydoc: resize done",
            ],
            Self::Workbench => &[
                "hk-server-display: composite ok",
                "hk-server-workbench: ready 1",
                "hk-server-workbench: documents ",
                "hk-server-workbench: open H:/Welcome",
                "hk-server-workbench: save ok",
                "hk-server-workbench: build ok",
                "hk-server-workbench: run 2",
                "hk-server-workbench: debug ",
                "hk-server-workbench: purge ok",
                "hk-server-workbench: done",
            ],
            Self::Debug => &[
                "hk-server-display: composite ok",
                "hk-server-debug: font scalable 16",
                "hk-server-debug: ready 1",
                "hk-server-debug: threads ",
                "hk-server-debug: objects ",
                "hk-server-debug: system-arena ",
                "hk-server-debug: symbol 0xffffffff80000000 [kernel] _start",
                "hk-server-debug: service symbol 0x",
                "[hk-server-debug]",
                "hk-server-debug: done",
            ],
            Self::DebugFault => &[
                "hk-server-display: composite ok",
                "hk-server-debug: ready 1",
                "hk-server-debug: fault symbolized",
                "hk-server-debug: done",
            ],
            Self::Taskman => &[
                "hk-server-display: composite ok",
                "hk-server-taskman: ready 1",
                "hk-server-taskman: cpus ",
                "hk-server-taskman: tasks ",
                "hk-server-taskman: rows ",
                "hk-server-taskman: cpu ",
                "hk-server-taskman: busy ",
                "hk-server-taskman: memory ",
                "hk-server-taskman: capacity-kinds ",
                "hk-server-taskman: system-arena ",
                "hk-server-taskman: render ok",
                "hk-server-taskman: done",
            ],
            Self::Dbg => &[
                "hk-server-dbg: ready",
                "hk-server-dbg: tasks",
                "hk-server-dbg: done",
            ],
            Self::DisplayRestart => &[
                "hk-server-display: restart requested",
                "hk-server-display: bye",
                "hk-server-console: reconnected",
                "hk-server-display-restart-test: restarted",
                "hk-server-console: done",
            ],
            Self::Hotswap => &["已替换：hk-server-echo", "hk-server-fault: 触发用户页错误"],
            Self::Checkpoint => &["已检查点：hk-server-memory", "已恢复：hk-server-memory"],
            Self::CheckpointServer => &[
                "hk-server-checkpoint: ready",
                "hk-server-checkpoint-test: saved ",
                "hk-server-checkpoint-test: restored ",
                "hk-server-checkpoint-test: listed ",
                "hk-server-checkpoint-test: done",
            ],
            Self::Memory => &["hk-server-memory-test: ok"],
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
            | Self::Tempo
            | Self::Wakeup
            | Self::Realtime
            | Self::Restart
            | Self::Ext4Root
            | Self::HolyFs
            | Self::Net
            | Self::NetStatic
            | Self::NetRestart
            | Self::NetZerocopy
            | Self::NetSocket
            | Self::Console
            | Self::Rng
            | Self::Vcon
            | Self::Display
            | Self::Terminal
            | Self::InputDriver
            | Self::Pointer
            | Self::Holydoc
            | Self::HolyDocResize
            | Self::Workbench
            | Self::Debug
            | Self::Taskman
            | Self::DebugFault
            | Self::Dbg
            | Self::Hotswap
            | Self::Checkpoint
            | Self::CheckpointServer
            | Self::DisplayRestart => QEMU_SUCCESS,
            Self::Smp => QEMU_SUCCESS,
            Self::Memory => QEMU_SUCCESS,
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
            Self::Tempo => "build/serial-tempo.log",
            Self::Wakeup => "build/serial-wakeup.log",
            Self::Realtime => "build/serial-realtime.log",
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
            Self::Rng => "build/serial-rng.log",
            Self::Vcon => "build/serial-vcon.log",
            Self::Display => "build/serial-display.log",
            Self::Terminal => "build/serial-terminal.log",
            Self::InputDriver => "build/serial-inputdriver.log",
            Self::Pointer => "build/serial-pointer.log",
            Self::Holydoc => "build/serial-holydoc.log",
            Self::HolyDocResize => "build/serial-holydocresize.log",
            Self::Workbench => "build/serial-workbench.log",
            Self::Debug => "build/serial-debug.log",
            Self::Taskman => "build/serial-taskman.log",
            Self::DebugFault => "build/serial-debugfault.log",
            Self::Dbg => "build/serial-dbg.log",
            Self::DisplayRestart => "build/serial-displayrestart.log",
            Self::Hotswap => "build/serial-hotswap.log",
            Self::Checkpoint => "build/serial-checkpoint.log",
            Self::CheckpointServer => "build/serial-checkpointsrv.log",
            Self::Memory => "build/serial-memory.log",
        }
    }

    /// 是否为网络模式（需要宿主 echo 与 QEMU 网卡）。
    fn is_network(self) -> bool {
        matches!(
            self,
            Self::Net | Self::NetStatic | Self::NetRestart | Self::NetZerocopy | Self::NetSocket
        )
    }

    /// 需要 QMP 通道注入输入或读取画面的模式。
    fn uses_qmp(self) -> bool {
        matches!(
            self,
            Self::Input
                | Self::Console
                | Self::Display
                | Self::Terminal
                | Self::InputDriver
                | Self::Pointer
                | Self::HolyDocResize
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

/// 单次冒烟的统一产物目录。
struct Artifacts {
    log: PathBuf,
    disk: PathBuf,
}

impl Artifacts {
    fn new(mode: SmokeMode) -> Result<Self> {
        let root = workspace::root();
        let suffix = std::env::var("HK_ISO_SUFFIX").unwrap_or_default();
        let stem = Path::new(mode.log_name())
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("boot");
        let key = std::env::var("HK_SMOKE_KEY").unwrap_or_else(|_| {
            let key = stem
                .strip_prefix("serial")
                .unwrap_or(stem)
                .trim_start_matches('-');
            let key = if key.is_empty() { "boot" } else { key };
            format!("{key}{suffix}")
        });
        let dir = root.join("build/smoke").join(key);
        fs::create_dir_all(&dir)
            .map_err(|error| Failure::new(format!("创建 {} 失败：{error}", dir.display())))?;
        let artifacts = Self {
            log: dir.join("serial.log"),
            disk: dir.join("disk.img"),
        };
        let _ = fs::remove_file(&artifacts.log);
        let _ = fs::remove_file(&artifacts.disk);
        Ok(artifacts)
    }

    /// 保留 `build/serial.log` 作为 boot 模式的稳定查看入口。
    fn mirror_boot_log(&self) {
        let target = workspace::root().join("build/serial.log");
        let _ = fs::copy(&self.log, target);
    }
}

/// QEMU 子进程守卫：任何提前返回或超时都会 kill 并回收。
struct QemuSession {
    child: Child,
}

impl QemuSession {
    fn new(child: Child) -> Self {
        Self { child }
    }

    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    fn kill_and_wait(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for QemuSession {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            self.kill_and_wait();
        }
    }
}

/// 启动内核并校验串口输出。
pub fn run(selection: Selection, release: bool, mode: SmokeMode) -> Result<()> {
    let targets = selection.targets();
    if targets.len() != 1 || targets[0] != Target::X86_64Qemu {
        return Err(Failure::new("smoke 只支持 x86_64-qemu"));
    }

    let (echo, tcp_port) = if mode.is_network() {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|error| Failure::new(format!("绑定 TCP echo 失败：{error}")))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| Failure::new(format!("设置 TCP echo 非阻塞失败：{error}")))?;
        let port = listener
            .local_addr()
            .map_err(|error| Failure::new(format!("读取 TCP echo 端口失败：{error}")))?
            .port();
        (Some(listener), port)
    } else {
        (None, 0)
    };
    let (udp, udp_port) = if mode.is_network() {
        let socket = UdpSocket::bind(("127.0.0.1", 0))
            .map_err(|error| Failure::new(format!("绑定 UDP echo 失败：{error}")))?;
        socket
            .set_read_timeout(Some(Duration::from_millis(100)))
            .map_err(|error| Failure::new(format!("设置 UDP echo 超时失败：{error}")))?;
        let port = socket
            .local_addr()
            .map_err(|error| Failure::new(format!("读取 UDP echo 端口失败：{error}")))?
            .port();
        (Some(socket), port)
    } else {
        (None, 0)
    };
    let net_ports = mode.is_network().then_some((tcp_port, udp_port));
    let cmdline = mode.cmdline(net_ports);
    let iso = crate::image::run(selection, release, cmdline.as_deref())?;
    let artifacts = Artifacts::new(mode)?;
    let disk = prepare_disk(mode, &artifacts.disk)?;
    let log = artifacts.log.clone();
    let socket = format!("127.0.0.1:{}", free_port()?);

    let vcon = if mode == SmokeMode::Vcon {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|error| Failure::new(format!("绑定 vcon echo 失败：{error}")))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| Failure::new(format!("设置 vcon echo 非阻塞失败：{error}")))?;
        let port = listener
            .local_addr()
            .map_err(|error| Failure::new(format!("读取 vcon echo 端口失败：{error}")))?
            .port();
        Some((listener, port))
    } else {
        None
    };

    let mut command = Command::new("qemu-system-x86_64");
    crate::qemu::accelerate(&mut command);
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
        command.args(["-netdev", "user,id=net0"]);
        command.args(["-device", "virtio-net-pci,netdev=net0"]);
    }
    if mode == SmokeMode::Rng {
        // Windows 上没有 `/dev/urandom`；`rng-builtin` 只是固定种子的伪随机，
        // 但冒烟校验的是 virtio-rng 驱动链路，不是熵质量。
        if cfg!(windows) {
            command.args(["-object", "rng-builtin,id=rng0"]);
        } else {
            command.args(["-object", "rng-random,id=rng0,filename=/dev/urandom"]);
        }
        command.args(["-device", "virtio-rng-pci,rng=rng0"]);
    }
    if let Some((_, port)) = &vcon {
        command
            .arg("-chardev")
            .arg(format!("socket,id=vcon,host=127.0.0.1,port={port}"))
            .args(["-device", "virtio-serial-pci,max_ports=1"])
            .args(["-device", "virtconsole,chardev=vcon"]);
    }
    if mode == SmokeMode::InputDriver {
        command.args(["-device", "virtio-keyboard-pci"]);
    }
    if matches!(mode, SmokeMode::Pointer | SmokeMode::HolyDocResize) {
        command.args(["-device", "virtio-tablet-pci"]);
    }
    if mode.uses_qmp() {
        command
            .arg("-qmp")
            .arg(format!("tcp:{socket},server=on,wait=off"));
    }

    let child = command
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| Failure::new(format!("启动 qemu-system-x86_64 失败：{error}")))?;
    let mut session = QemuSession::new(child);

    let vcon_peer = vcon.map(|(listener, _)| thread::spawn(move || serve_vcon_echo(listener)));

    if mode == SmokeMode::Input {
        // 等内核 Shell 打出提示符再注入；启动耗时随机器负载变化，固定等待会丢按键。
        wait_for(&log, "hk> ", Duration::from_secs(30))?;
        inject(&socket, "echo hello-ioapic\n")?;
        thread::sleep(Duration::from_millis(500));
        inject(&socket, "use boot.hsh\n")?;
        thread::sleep(Duration::from_millis(500));
        inject(&socket, "exit\n")?;
    }
    if mode == SmokeMode::Console {
        wait_for(
            &log,
            "hk-server-console-test: type a line",
            Duration::from_secs(15),
        )?;
        inject(&socket, "hello\n")?;
    }
    if mode == SmokeMode::Display {
        wait_for(
            &log,
            "hk-server-display-test: ready",
            Duration::from_secs(15),
        )?;
        inject_key(&socket, "tab")?;
        thread::sleep(Duration::from_millis(300));
        inject(&socket, "a")?;
    }
    if mode == SmokeMode::Terminal {
        wait_for(&log, "hk-server-console: sgr ok", Duration::from_secs(15))?;
        inject(&socket, "hi\n")?;
    }
    if mode == SmokeMode::InputDriver {
        wait_for(&log, "hk-server-console: sgr ok", Duration::from_secs(20))?;
        inject(&socket, "hi\n")?;
    }
    if mode == SmokeMode::Pointer {
        wait_for(
            &log,
            "hk-server-pointer-test: ready",
            Duration::from_secs(15),
        )?;
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
        // 窗口正文中心约在 (736,410)：验证指针移动事件与正文点击。
        inject_pointer(&socket, 0x4997, 0x4199)?;
        thread::sleep(Duration::from_millis(300));
        // 还原后窗口右上方约在 (958,236) 的关闭按钮。
        inject_pointer(&socket, 0x5FCC, 0x25C2)?;
    }
    if mode == SmokeMode::HolyDocResize {
        wait_for(&log, "hk-server-holydoc: ready 1", Duration::from_secs(30))?;
        // 760x520 窗口位于 260,114；标题栏最大化按钮约在 (995,126)。
        inject_pointer(&socket, 0x6380, 0x1429)?;
    }
    let udp_echo = udp.map(|socket| thread::spawn(move || serve_udp_echo(socket)));
    if let Some(listener) = echo {
        serve_tcp_echo(listener)?;
    }

    let timeout = if matches!(
        mode,
        SmokeMode::Holydoc | SmokeMode::HolyDocResize | SmokeMode::Workbench
    ) {
        HOLYDOC_TIMEOUT
    } else if mode == SmokeMode::Shell {
        SHELL_TIMEOUT
    } else {
        TIMEOUT
    };
    let deadline = Instant::now() + timeout;
    let status = loop {
        match session.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() > deadline => {
                session.kill_and_wait();
                break None;
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(error) => return Err(Failure::new(format!("等待 QEMU 失败：{error}"))),
        }
    };

    if let Some(handle) = udp_echo {
        let _ = handle.join();
    }
    if let Some(handle) = vcon_peer {
        handle
            .join()
            .map_err(|_| Failure::new("vcon echo 线程崩溃"))??;
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
    if mode == SmokeMode::Boot {
        artifacts.mirror_boot_log();
    }
    let expected_code = mode.exit_code();
    match status {
        Some(status) if status.code() == Some(expected_code) => {}
        Some(status) => {
            return Err(Failure::new(format!(
                "QEMU 退出码 {:?}，期望 {expected_code}\n串口输出：\n{}",
                status.code(),
                serial_tail(&text)
            )));
        }
        None => {
            return Err(Failure::new(format!(
                "QEMU 超时未退出\n串口输出：\n{}",
                serial_tail(&text)
            )));
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

/// 接受 QEMU 的 virtio-console 连接，等待测试行并回显确认。
fn serve_vcon_echo(listener: TcpListener) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() > deadline {
                    return Err(Failure::new("QEMU 未连接 vcon echo"));
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(Failure::new(format!("vcon echo accept 失败：{error}"))),
        }
    };
    stream
        .set_nonblocking(false)
        .map_err(|error| Failure::new(format!("设置 vcon echo 阻塞失败：{error}")))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| Failure::new(format!("设置 vcon echo 超时失败：{error}")))?;

    let mut seen = Vec::new();
    let mut buffer = [0_u8; 64];
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => return Err(Failure::new("vcon echo 连接提前关闭")),
            Ok(length) => {
                seen.extend_from_slice(&buffer[..length]);
                if seen
                    .windows(b"vcon hello\n".len())
                    .any(|window| window == b"vcon hello\n")
                {
                    break;
                }
                if seen.len() > 4096 {
                    return Err(Failure::new("vcon echo 收到过多数据"));
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if Instant::now() > deadline {
                    return Err(Failure::new("等待 vcon guest 写入超时"));
                }
            }
            Err(error) => return Err(Failure::new(format!("vcon echo 读取失败：{error}"))),
        }
    }
    stream
        .write_all(b"vcon-ack\n")
        .map_err(|error| Failure::new(format!("vcon echo 写回失败：{error}")))?;
    Ok(())
}

/// 接受一次 QEMU 宿主的 TCP 连接并回显。
fn serve_tcp_echo(listener: TcpListener) -> Result<()> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                // Windows 上 accept 返回的套接字继承监听端的非阻塞状态，Linux 不会；
                // 先切回阻塞，读超时才生效。
                stream
                    .set_nonblocking(false)
                    .map_err(|error| Failure::new(format!("设置 TCP echo 阻塞失败：{error}")))?;
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
    let deadline = Instant::now() + TIMEOUT;
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

/// 截取串口日志尾部，避免超时错误淹没输出。
fn serial_tail(text: &str) -> String {
    const LIMIT: usize = 8000;
    if text.len() <= LIMIT {
        return text.to_string();
    }
    let mut start = text.len() - LIMIT;
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    format!("…（前文省略）\n{}", &text[start..])
}

/// 等待串口日志出现指定输出。
fn wait_for(log: &Path, token: &str, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let text = fs::read(log).unwrap_or_default();
        if String::from_utf8_lossy(&text).contains(token) {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(Failure::new(format!("等待串口输出超时：{token}")));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// 预留一个空闲的环回端口：绑定到 0 取号后立即释放，交给 QEMU 监听。
fn free_port() -> Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|error| Failure::new(format!("绑定 QMP 端口失败：{error}")))?;
    let port = listener
        .local_addr()
        .map_err(|error| Failure::new(format!("读取 QMP 端口失败：{error}")))?
        .port();
    Ok(port)
}

/// 生成新的测试磁盘。
fn prepare_disk(mode: SmokeMode, path: &Path) -> Result<PathBuf> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| Failure::new(format!("创建 {} 失败：{error}", parent.display())))?;
    }
    let blocks = crate::disk::DISK_BYTES / SECTOR_SIZE as u64;
    let disk = MemoryDisk::new(blocks, SECTOR_SIZE);
    let disk = if matches!(
        mode,
        SmokeMode::HolyFs
            | SmokeMode::Holydoc
            | SmokeMode::HolyDocResize
            | SmokeMode::Workbench
            | SmokeMode::CheckpointServer
    ) {
        crate::disk::seed_holy_docs(disk)?
    } else {
        hk_fs_ext4::format_with_wall_clock(disk, HostClock)
            .map_err(|error| Failure::new(format!("格式化 ext4 测试磁盘失败：{error}")))?
    };
    fs::write(path, disk.into_data())
        .map_err(|error| Failure::new(format!("写入 {} 失败：{error}", path.display())))?;
    Ok(path.to_path_buf())
}

/// 从磁盘镜像重新挂载 HolyFS，确认标记文件已经持久化。
fn verify_holyfs(path: &Path) -> Result<()> {
    let bytes = fs::read(path)
        .map_err(|error| Failure::new(format!("读取 {} 失败：{error}", path.display())))?;
    let disk = MemoryDisk::from_data(bytes, SECTOR_SIZE)
        .ok_or_else(|| Failure::new("测试磁盘长度不是扇区整数倍"))?;
    let half = disk.capacity_blocks() / 2;
    let volume = hk_fs_holyfs::HolyFs::mount(DiskSlice::new(disk, 0, half))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serial_tail_keeps_the_end() {
        let text = "a".repeat(9000) + "tail";
        let tail = serial_tail(&text);
        assert!(tail.ends_with("tail"));
        assert!(tail.len() < text.len());
        assert!(tail.starts_with('…'));
    }

    #[test]
    fn serial_tail_handles_utf8_boundaries() {
        let text = "测".repeat(3000);
        let tail = serial_tail(&text);
        assert!(tail.len() < text.len());
        let body = tail.strip_prefix("…（前文省略）\n").expect("截断标记");
        assert!(body.chars().all(|character| character == '测'));
    }
}
