//! QMP 按键注入：把文本转成 `send-key` 命令。

use std::path::Path;
#[cfg(unix)]
use std::thread;
#[cfg(unix)]
use std::time::{Duration, Instant};

use crate::failure::{Failure, Result};

/// 通过 QMP 套接字注入一串按键。
#[cfg(unix)]
pub(super) fn inject(socket: &Path, text: &str) -> Result<()> {
    inject_codes(socket, &qcodes(text))
}

/// 通过 QMP 套接字注入一个 qcode（例如 `tab`）。
#[cfg(unix)]
pub(super) fn inject_key(socket: &Path, code: &str) -> Result<()> {
    inject_codes(socket, &[String::from(code)])
}

/// 通过 QMP 注入绝对指针移动与左键点击。
#[cfg(unix)]
pub(super) fn inject_pointer(socket: &Path, x: u64, y: u64) -> Result<()> {
    send_pointer_events(
        socket,
        &format!(
            "{},{{\"type\":\"btn\",\"data\":{{\"down\":true,\"button\":\"left\"}}}},{{\"type\":\"btn\",\"data\":{{\"down\":false,\"button\":\"left\"}}}}",
            absolute(x, y)
        ),
    )
}

/// 通过 QMP 注入标题栏拖拽：按下、移动、释放。
#[cfg(unix)]
pub(super) fn inject_pointer_drag(
    socket: &Path,
    from_x: u64,
    from_y: u64,
    to_x: u64,
    to_y: u64,
) -> Result<()> {
    send_pointer_events(
        socket,
        &format!(
            "{},{{\"type\":\"btn\",\"data\":{{\"down\":true,\"button\":\"left\"}}}}",
            absolute(from_x, from_y)
        ),
    )?;
    thread::sleep(Duration::from_millis(150));
    send_pointer_events(socket, &absolute(to_x, to_y))?;
    thread::sleep(Duration::from_millis(150));
    send_pointer_events(
        socket,
        "{\"type\":\"btn\",\"data\":{\"down\":false,\"button\":\"left\"}}",
    )
}

/// 构造绝对坐标事件。
#[cfg(unix)]
fn absolute(x: u64, y: u64) -> String {
    format!(
        "{{\"type\":\"abs\",\"data\":{{\"axis\":\"x\",\"value\":{x}}}}},{{\"type\":\"abs\",\"data\":{{\"axis\":\"y\",\"value\":{y}}}}}"
    )
}

/// 通过 QMP 发送一组 `input-send-event` 事件。
#[cfg(unix)]
fn send_pointer_events(socket: &Path, events: &str) -> Result<()> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    let deadline = Instant::now() + Duration::from_secs(5);
    let stream = loop {
        match UnixStream::connect(socket) {
            Ok(stream) => break stream,
            Err(_) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(Failure::new(format!("连接 QMP 失败：{error}"))),
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| Failure::new(format!("设置 QMP 超时失败：{error}")))?;
    let mut writer = stream
        .try_clone()
        .map_err(|error| Failure::new(format!("复制 QMP 连接失败：{error}")))?;
    let mut reader = BufReader::new(stream);
    let mut greeting = String::new();
    reader
        .read_line(&mut greeting)
        .map_err(|error| Failure::new(format!("读取 QMP 问候失败：{error}")))?;
    writer
        .write_all(b"{\"execute\":\"qmp_capabilities\"}\n")
        .and_then(|()| writer.flush())
        .map_err(|error| Failure::new(format!("发送 QMP 握手失败：{error}")))?;
    thread::sleep(Duration::from_millis(100));

    let command =
        format!("{{\"execute\":\"input-send-event\",\"arguments\":{{\"events\":[{events}]}}}}\n");
    writer
        .write_all(command.as_bytes())
        .and_then(|()| writer.flush())
        .map_err(|error| Failure::new(format!("发送指针事件失败：{error}")))?;
    thread::sleep(Duration::from_millis(100));
    Ok(())
}

#[cfg(unix)]
fn inject_codes(socket: &Path, codes: &[String]) -> Result<()> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    let deadline = Instant::now() + Duration::from_secs(5);
    let stream = loop {
        match UnixStream::connect(socket) {
            Ok(stream) => break stream,
            Err(_) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(Failure::new(format!("连接 QMP 失败：{error}"))),
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| Failure::new(format!("设置 QMP 超时失败：{error}")))?;

    let mut writer = stream
        .try_clone()
        .map_err(|error| Failure::new(format!("复制 QMP 连接失败：{error}")))?;
    let mut reader = BufReader::new(stream);
    let mut greeting = String::new();
    reader
        .read_line(&mut greeting)
        .map_err(|error| Failure::new(format!("读取 QMP 问候失败：{error}")))?;

    writer
        .write_all(b"{\"execute\":\"qmp_capabilities\"}\n")
        .and_then(|()| writer.flush())
        .map_err(|error| Failure::new(format!("发送 QMP 握手失败：{error}")))?;
    thread::sleep(Duration::from_millis(100));

    for key in codes {
        let command = format!(
            "{{\"execute\":\"send-key\",\"arguments\":{{\"keys\":[{{\"type\":\"qcode\",\"data\":\"{key}\"}}]}}}}\n"
        );
        writer
            .write_all(command.as_bytes())
            .and_then(|()| writer.flush())
            .map_err(|error| Failure::new(format!("发送按键失败：{error}")))?;
        thread::sleep(Duration::from_millis(40));
    }
    Ok(())
}

/// 非 Unix 主机不支持 QMP 套接字。
#[cfg(not(unix))]
pub(super) fn inject(_socket: &Path, _text: &str) -> Result<()> {
    Err(Failure::new("键盘输入冒烟只在 Unix 主机上运行"))
}

/// 非 Unix 主机不支持 QMP 套接字。
#[cfg(not(unix))]
pub(super) fn inject_key(_socket: &Path, _code: &str) -> Result<()> {
    Err(Failure::new("键盘输入冒烟只在 Unix 主机上运行"))
}

/// 非 Unix 主机不支持 QMP 套接字。
#[cfg(not(unix))]
pub(super) fn inject_pointer(_socket: &Path, _x: u64, _y: u64) -> Result<()> {
    Err(Failure::new("指针输入冒烟只在 Unix 主机上运行"))
}

/// 非 Unix 主机不支持 QMP 套接字。
#[cfg(not(unix))]
pub(super) fn inject_pointer_drag(
    _socket: &Path,
    _from_x: u64,
    _from_y: u64,
    _to_x: u64,
    _to_y: u64,
) -> Result<()> {
    Err(Failure::new("指针输入冒烟只在 Unix 主机上运行"))
}

#[cfg(any(unix, test))]
fn qcodes(text: &str) -> Vec<String> {
    text.chars()
        .map(|character| match character {
            'a'..='z' | '0'..='9' => character.to_string(),
            ' ' => String::from("spc"),
            '-' => String::from("minus"),
            '.' => String::from("dot"),
            '/' => String::from("slash"),
            '\n' | '\r' => String::from("ret"),
            _ => String::from("spc"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_qcodes_stable() {
        assert_eq!(qcodes("a b-1"), vec!["a", "spc", "b", "minus", "1"]);
        assert_eq!(qcodes("a.b/c"), vec!["a", "dot", "b", "slash", "c"]);
    }
}
