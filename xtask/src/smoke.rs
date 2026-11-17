//! QEMU 冒烟测试：启动内核并检查串口输出。

mod net;
mod qmp;
mod run;
mod verify;

pub use run::run;

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

/// 冒烟预算：慢速宿主（WHPX/TCG）可用 `HK_SMOKE_TIMEOUT=<秒>` 整体放宽。
fn smoke_timeout(default: Duration) -> Duration {
    std::env::var("HK_SMOKE_TIMEOUT")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .map(Duration::from_secs)
        .unwrap_or(default)
}

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
    /// virtio-gpu 扫描输出与诊断图案
    #[value(name = "gpu")]
    Gpu,
    /// virtio-sound 播放自检音（WAV 后端）
    #[value(name = "sound")]
    Sound,
    /// 只读 virtio-blk 设备属性
    #[value(name = "blkro")]
    BlockReadOnly,
    /// virtio-console 驱动与宿主回显
    #[value(name = "vcon")]
    Vcon,
    /// virtio-console 多端口（port 1 走 virtserialport）
    #[value(name = "vconmulti")]
    VconMulti,
    /// Display 服务帧缓冲映射与测试图案
    #[value(name = "display")]
    Display,
    /// Display 接管 virtio-gpu 扫描输出
    #[value(name = "gpudisplay")]
    GpuDisplay,
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
    /// 修订能力映射与 VFS Provider
    #[value(name = "revmap")]
    Revmap,
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
    /// Provider 压力自动换出/换入
    #[value(name = "swap")]
    Swap,
    /// 懒 TaskImage 从模块镜像缺页装入
    #[value(name = "task-image")]
    TaskImage,
    /// 用户态 Image Server 提供懒 TaskImage 页
    #[value(name = "task-image-user")]
    TaskImageUser,

    /// 内核堆超过启动期 16 MiB 后按需增长
    #[value(name = "heap")]
    Heap,
    /// OOM 阈值、victim 选择与审计
    #[value(name = "oom")]
    Oom,
    /// 多 NUMA 节点拓扑与节点容量分配
    #[value(name = "numa")]
    Numa,
    /// 页从节点 0 迁移到节点 1
    #[value(name = "numa-migrate")]
    NumaMigrate,
    /// Gpu DeviceAllocation 与 Provider 驻留
    #[value(name = "gpualloc")]
    GpuAlloc,
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
            Self::Gpu => Some(String::from("gpu-test")),
            Self::Sound => Some(String::from("sound-test")),
            Self::BlockReadOnly => Some(String::from("rng-test")),
            Self::Vcon | Self::VconMulti => Some(String::from("vcon-test")),
            Self::Display => Some(String::from("theme=light ui.scale=2 display-test")),
            Self::GpuDisplay => Some(String::from(
                "theme=light ui.scale=2 display-test gpu.scanout",
            )),
            Self::Terminal => Some(String::from("terminal-test")),
            Self::InputDriver => Some(String::from("input-test input-driver terminal-test")),
            Self::Pointer => Some(String::from("pointer-test input-driver")),
            Self::Holydoc => Some(String::from("vfs.fs=holyfs holydoc-test")),
            Self::Revmap => Some(String::from("vfs.fs=holyfs holydoc-test revmap-test")),
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
            Self::Swap => Some(String::from("swap-test")),
            Self::TaskImage => Some(String::from("task-image")),
            Self::TaskImageUser => Some(String::from("task-image-user image.user")),
            Self::Heap => Some(String::from("shell-test heap-test")),
            Self::Oom => Some(String::from("oom-test")),
            Self::Numa => None,
            Self::NumaMigrate => Some(String::from("numa-test")),
            Self::GpuAlloc => Some(String::from("gpu-alloc-test")),
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
                "hk-server-virtio: block msi-x bound",
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
                "hk-server-virtio: block msi-x bound",
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
                "hk-client-netstat: mac=",
                "hk-client-netstat: rx_frames=",
                "hk-client-netstat: route=",
                "hk-client-netstat: arp",
                "hk-client-netstat: config dhcp=",
            ],
            Self::NetStatic => &[
                "hk-server-virtio-net: mac=",
                "hk-server-net: static ok",
                "hk-server-net: ping ok",
                "hk-server-net: tcp ok",
                "hk-server-net: udp ok",
                "hk-server-net: dns ok",
                "127.0.0.1",
                "hk-client-netstat: config dhcp=off",
                "hk-client-netstat: config dns=10.0.2.3",
                "hk-client-netstat: route=",
                "hk-client-netstat: arp",
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
                "hk-client-netstat: config dhcp=",
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
                "hk-client-netstat: config dhcp=",
            ],
            Self::NetSocket => &[
                "hk-server-net: ready",
                "hk-client-netsock: tcp ok",
                "hk-client-netsock: udp ok",
                "hk-client-netsock: dns ok",
            ],
            Self::Console => &["hk-client-console-test: got hello"],
            Self::Rng => &[
                "hk-server-virtio: entropy ok",
                "hk-server-virtio: rng ready",
            ],
            Self::Gpu => &[
                "hk-server-virtio: gpu ready",
                "hk-server-virtio: gpu edid 1280x800",
                "hk-server-virtio: gpu 1280x800 ok",
            ],
            Self::Sound => &[
                "hk-server-virtio-sound: ready streams 2",
                "hk-client-sound-test: info streams 2",
                "hk-client-sound-test: concurrent 660+880 ok",
                "hk-client-sound-test: async accept",
            ],
            Self::BlockReadOnly => &["hk-server-virtio: virtio-blk sectors=", " readonly"],
            Self::Vcon => &[
                "hk-server-virtio: console ready",
                "hk-server-virtio: console echo ok",
            ],
            Self::VconMulti => &[
                "hk-server-virtio: console ready ports 2",
                "hk-server-virtio: console echo ok",
                "hk-server-virtio: console port 1 ok",
            ],
            Self::Display => &[
                "hk-server-display: font scalable 32",
                "hk-server-display: theme light",
                "hk-server-display: scale 2",
                "hk-server-display: ready",
                "hk-server-display: title ok",
                "hk-server-display: composite ok",
                "hk-server-display: back provider ok",
                "hk-server-display: surface ",
                "hk-server-display: clipboard set",
                "hk-server-display: clipboard get",
                "hk-server-display: focus 1",
                "hk-client-display-test: clipboard holy",
                "hk-client-display-test: ready",
                "hk-client-display-test: key 97 win 1",
                "hk-client-display-test: done",
                "hk-server-display: scanout 1 on",
                "hk-server-display: scanout 2 on",
            ],
            Self::GpuDisplay => &[
                "hk-server-display: scanout gpu 1280x800",
                "hk-server-display: ready 1280x800",
                "hk-server-display: composite ok",
                "hk-client-display-test: clipboard holy",
                "hk-client-display-test: ready",
                "hk-client-display-test: done",
            ],
            Self::Terminal => &[
                "hk-server-console: window",
                "hk-server-console: utf8 ok",
                "hk-server-console: cjk ok",
                "hk-server-console: sgr ok",
                "hk-server-display: composite ok",
                "hk-client-terminal-test: line hi",
                "hk-client-terminal-test: done",
            ],
            Self::InputDriver => &[
                "hk-server-input: ready",
                "hk-server-display: composite ok",
                "hk-client-terminal-test: line hi",
                "hk-client-terminal-test: done",
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
                "hk-client-pointer-test: resize 464x348",
                "hk-server-display: maximize 1 1280x748",
                "hk-server-display: restore 1 464x348",
                "hk-server-display: minimize 1",
                "hk-server-display: scanout 1 off",
                "hk-client-pointer-test: pointer 1",
                "hk-server-display: pointer click 1",
                "hk-client-pointer-test: clicked",
                "hk-server-display: close 1",
                "hk-client-pointer-test: closed 1",
            ],
            Self::Revmap => &[
                "hk-server-vfs: revision shared",
                "hk-server-holydoc: revision ok",
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
                "hk-server-vfs: revision shared",
                "hk-server-holydoc: revision ok",
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
                "hk-client-dbg: ready",
                "hk-client-dbg: tasks",
                "hk-client-dbg: done",
            ],
            Self::DisplayRestart => &[
                "hk-server-display: restart requested",
                "hk-server-display: bye",
                "hk-server-console: reconnected",
                "hk-client-display-restart-test: restarted",
                "hk-server-console: done",
            ],
            Self::Hotswap => &["已替换：hk-server-echo", "hk-server-fault: 触发用户页错误"],
            Self::Checkpoint => &["已检查点：hk-server-memory", "已恢复：hk-server-memory"],
            Self::CheckpointServer => &[
                "hk-server-checkpoint: ready",
                "hk-client-checkpoint-test: saved ",
                "hk-client-checkpoint-test: restored ",
                "hk-client-checkpoint-test: listed ",
                "hk-client-checkpoint-test: done",
            ],
            Self::Memory => &["hk-server-memory-test: ok"],
            Self::Swap => &[
                "hk-client-swap-test: pages ",
                " swapped ",
                "hk-client-swap-test: verified ",
                "hk-client-swap-test: ok",
            ],
            Self::TaskImage => &["hk-client-image-test: lazy-ok"],
            Self::TaskImageUser => &["hk-client-image-test: lazy-ok"],
            Self::Heap => &["内核堆测试：24 MiB ok"],
            Self::Oom => &[
                "hk-client-oom-test: triggering",
                "hk-client-oom-observer: victim ",
                "hk-client-oom-observer: events ",
                "hk-client-oom-observer: ok",
            ],
            Self::Numa => Self::Boot.expected(),
            Self::NumaMigrate => &["hk-client-numa-test: migrated ", "hk-client-numa-test: ok"],
            Self::GpuAlloc => &[
                "hk-server-gpu-provider: ready",
                "hk-client-gpu-test: provider page ok",
                "hk-client-gpu-test: gpu bytes ",
                "hk-client-gpu-test: ok",
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
            | Self::Gpu
            | Self::BlockReadOnly
            | Self::Vcon
            | Self::VconMulti
            | Self::Sound
            | Self::Display
            | Self::GpuDisplay
            | Self::Terminal
            | Self::InputDriver
            | Self::Pointer
            | Self::Holydoc
            | Self::Revmap
            | Self::HolyDocResize
            | Self::Workbench
            | Self::Debug
            | Self::Taskman
            | Self::DebugFault
            | Self::Dbg
            | Self::Hotswap
            | Self::Checkpoint
            | Self::CheckpointServer
            | Self::DisplayRestart
            | Self::Numa
            | Self::NumaMigrate
            | Self::GpuAlloc
            | Self::TaskImage
            | Self::TaskImageUser
            | Self::Swap
            | Self::Heap
            | Self::Oom => QEMU_SUCCESS,
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
            Self::Gpu => "build/serial-gpu.log",
            Self::Sound => "build/serial-sound.log",
            Self::BlockReadOnly => "build/serial-blkro.log",
            Self::Vcon => "build/serial-vcon.log",
            Self::VconMulti => "build/serial-vconmulti.log",
            Self::Display => "build/serial-display.log",
            Self::GpuDisplay => "build/serial-gpudisplay.log",
            Self::Terminal => "build/serial-terminal.log",
            Self::InputDriver => "build/serial-inputdriver.log",
            Self::Pointer => "build/serial-pointer.log",
            Self::Holydoc => "build/serial-holydoc.log",
            Self::Revmap => "build/serial-revmap.log",
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
            Self::Swap => "build/serial-swap.log",
            Self::TaskImage => "build/serial-task-image.log",
            Self::TaskImageUser => "build/serial-task-image-user.log",
            Self::Heap => "build/serial-heap.log",
            Self::Oom => "build/serial-oom.log",
            Self::Numa => "build/serial-numa.log",
            Self::NumaMigrate => "build/serial-numa-migrate.log",
            Self::GpuAlloc => "build/serial-gpualloc.log",
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
                | Self::GpuDisplay
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
