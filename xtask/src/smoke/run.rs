//! 冒烟执行：启动 QEMU、注入输入并校验结果。

use std::fs;
use std::net::{TcpListener, UdpSocket};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::net::{free_port, serve_port_echo, serve_tcp_echo, serve_udp_echo, serve_vcon_echo};
use super::qmp::{inject, inject_key, inject_pointer, inject_pointer_drag};
use super::verify::{prepare_disk, serial_tail, verify_ext4, verify_holyfs, verify_tone, wait_for};
use super::{
    Artifacts, HOLYDOC_TIMEOUT, QemuSession, SHELL_TIMEOUT, SmokeMode, TIMEOUT, smoke_timeout,
};
use crate::failure::{Failure, Result};
use crate::target::{Selection, Target};
use crate::workspace;

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

    let vcon = if matches!(mode, SmokeMode::Vcon | SmokeMode::VconMulti) {
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

    let vcon_extra = if mode == SmokeMode::VconMulti {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .map_err(|error| Failure::new(format!("绑定 vcon 1 echo 失败：{error}")))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| Failure::new(format!("设置 vcon 1 echo 非阻塞失败：{error}")))?;
        let port = listener
            .local_addr()
            .map_err(|error| Failure::new(format!("读取 vcon 1 echo 端口失败：{error}")))?
            .port();
        Some((listener, port))
    } else {
        None
    };

    let tone = workspace::root().join("build/smoke/sound/tone.wav");
    if mode == SmokeMode::Sound {
        if let Some(parent) = tone.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::remove_file(&tone);
    }

    let mut command = Command::new(crate::qemu::program());
    crate::qemu::accelerate(&mut command);
    command.args(["-M", "q35"]);
    if matches!(mode, SmokeMode::Numa | SmokeMode::NumaMigrate) {
        command.args([
            "-m",
            "256M",
            "-object",
            "memory-backend-ram,id=mem0,size=128M",
            "-object",
            "memory-backend-ram,id=mem1,size=128M",
            "-numa",
            "node,nodeid=0,memdev=mem0",
            "-numa",
            "node,nodeid=1,memdev=mem1",
        ]);
    } else {
        command.args(["-m", "256M"]);
    }
    command
        .arg("-cdrom")
        .arg(&iso)
        .args(["-boot", "d", "-display", "none", "-no-reboot", "-serial"])
        .arg(format!("file:{}", log.display()))
        .arg("-drive")
        .arg(if mode == SmokeMode::BlockReadOnly {
            format!(
                "file={},if=none,id=disk,format=raw,readonly=on",
                disk.display()
            )
        } else {
            format!("file={},if=none,id=disk,format=raw", disk.display())
        })
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
    if mode == SmokeMode::Gpu {
        // 保留默认 VGA 供 Limine 建立帧缓冲，另加一块 virtio-gpu 供驱动自检。
        command.args(["-device", "virtio-gpu-pci"]);
    }
    if mode == SmokeMode::Sound {
        // WAV 后端把播放结果落盘，冒烟结束后解析音高。
        command
            .arg("-audiodev")
            .arg(format!("wav,id=snd0,path={}", tone.display()))
            .args(["-device", "virtio-sound-pci,audiodev=snd0,streams=4"]);
    }
    if mode == SmokeMode::GpuDisplay {
        // 没有 VGA：Display 只能从 virtio-gpu 接管扫描输出。
        command.args(["-vga", "none", "-device", "virtio-gpu-pci"]);
    }
    if let Some((_, port)) = &vcon {
        let ports = if mode == SmokeMode::VconMulti { 2 } else { 1 };
        command
            .arg("-chardev")
            .arg(format!("socket,id=vcon,host=127.0.0.1,port={port}"))
            .arg("-device")
            .arg(format!("virtio-serial-pci,max_ports={ports}"))
            .args(["-device", "virtconsole,chardev=vcon"]);
        if let Some((_, extra)) = &vcon_extra {
            command
                .arg("-chardev")
                .arg(format!("socket,id=vcon1,host=127.0.0.1,port={extra}"))
                .args(["-device", "virtserialport,chardev=vcon1,name=vport1"]);
        }
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
        .map_err(|error| crate::qemu::launch_failure(&error))?;
    let mut session = QemuSession::new(child);

    let vcon_peer = vcon.map(|(listener, _)| thread::spawn(move || serve_vcon_echo(listener)));
    let vcon_extra_peer =
        vcon_extra.map(|(listener, _)| thread::spawn(move || serve_port_echo(listener)));

    if mode == SmokeMode::Numa {
        wait_for(&log, "NUMA 物理记录：", Duration::from_secs(30))?;
        wait_for(
            &log,
            "hk-server-memory: numa records ",
            Duration::from_secs(30),
        )?;
        wait_for(&log, "NUMA 节点分配 ok", Duration::from_secs(30))?;
    }
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
            "hk-client-console-test: type a line",
            Duration::from_secs(15),
        )?;
        inject(&socket, "hello\n")?;
    }
    if matches!(mode, SmokeMode::Display | SmokeMode::GpuDisplay) {
        wait_for(
            &log,
            "hk-client-display-test: ready",
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
            "hk-client-pointer-test: ready",
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

    let timeout = smoke_timeout(
        if matches!(
            mode,
            SmokeMode::Holydoc | SmokeMode::HolyDocResize | SmokeMode::Workbench
        ) {
            HOLYDOC_TIMEOUT
        } else if mode == SmokeMode::Shell {
            SHELL_TIMEOUT
        } else {
            TIMEOUT
        },
    );
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
    if let Some(handle) = vcon_extra_peer {
        handle
            .join()
            .map_err(|_| Failure::new("vcon 1 echo 线程崩溃"))??;
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

    if mode == SmokeMode::Sound {
        verify_tone(&tone)?;
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
