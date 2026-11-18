//! 宿主侧回显服务：TCP/UDP/virtio-console 与空闲端口预留。

use std::io::{Read, Write};
use std::net::{TcpListener, UdpSocket};
use std::thread;
use std::time::{Duration, Instant};

use super::{TIMEOUT, smoke_timeout};
use crate::failure::{Failure, Result};

/// 接受 QEMU 的 virtio-console 连接，等待测试行并回显确认。
pub(super) fn serve_port_echo(listener: TcpListener) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() > deadline {
                    return Err(Failure::new("QEMU 未连接 vcon 1 echo"));
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(Failure::new(format!("vcon 1 echo accept 失败：{error}"))),
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| Failure::new(format!("设置 vcon 1 echo 超时失败：{error}")))?;
    let mut seen = Vec::new();
    let mut buffer = [0_u8; 64];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => return Err(Failure::new("vcon 1 echo 连接提前关闭")),
            Ok(length) => {
                seen.extend_from_slice(&buffer[..length]);
                stream
                    .write_all(&buffer[..length])
                    .map_err(|error| Failure::new(format!("vcon 1 echo 写回失败：{error}")))?;
                if seen.windows(b"vcon p1\n".len()).any(|w| w == b"vcon p1\n") {
                    return Ok(());
                }
                if seen.len() > 4096 {
                    return Err(Failure::new("vcon 1 echo 收到过多数据"));
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if Instant::now() > deadline + Duration::from_secs(30) {
                    return Err(Failure::new("vcon 1 echo 超时"));
                }
            }
            Err(error) => return Err(Failure::new(format!("vcon 1 echo 读取失败：{error}"))),
        }
    }
}

pub(super) fn serve_vcon_echo(listener: TcpListener) -> Result<()> {
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
pub(super) fn serve_tcp_echo(listener: TcpListener) -> Result<()> {
    let deadline = Instant::now() + smoke_timeout(TIMEOUT);
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
pub(super) fn serve_udp_echo(socket: UdpSocket) -> Result<()> {
    let deadline = Instant::now() + smoke_timeout(TIMEOUT);
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

/// 预留一个空闲的环回端口：绑定到 0 取号后立即释放，交给 QEMU 监听。
pub(super) fn free_port() -> Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|error| Failure::new(format!("绑定 QMP 端口失败：{error}")))?;
    let port = listener
        .local_addr()
        .map_err(|error| Failure::new(format!("读取 QMP 端口失败：{error}")))?
        .port();
    Ok(port)
}
