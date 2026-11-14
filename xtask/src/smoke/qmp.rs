//! QMP 按键注入：把文本转成 `send-key` 命令。

use std::net::TcpStream;
use std::thread;
use std::time::{Duration, Instant};

use crate::failure::{Failure, Result};

/// 通过 QMP 套接字注入一串按键。
pub(super) fn inject(socket: &str, text: &str) -> Result<()> {
    inject_codes(socket, &qcodes(text))
}

/// 通过 QMP 套接字注入一个 qcode（例如 `tab`）。
pub(super) fn inject_key(socket: &str, code: &str) -> Result<()> {
    inject_codes(socket, &[String::from(code)])
}

/// 通过 QMP 注入绝对指针移动与左键点击。
pub(super) fn inject_pointer(socket: &str, x: u64, y: u64) -> Result<()> {
    send_pointer_events(
        socket,
        &format!(
            "{},{{\"type\":\"btn\",\"data\":{{\"down\":true,\"button\":\"left\"}}}},{{\"type\":\"btn\",\"data\":{{\"down\":false,\"button\":\"left\"}}}}",
            absolute(x, y)
        ),
    )
}

/// 通过 QMP 注入标题栏拖拽：按下、移动、释放。
pub(super) fn inject_pointer_drag(
    socket: &str,
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
fn absolute(x: u64, y: u64) -> String {
    format!(
        "{{\"type\":\"abs\",\"data\":{{\"axis\":\"x\",\"value\":{x}}}}},{{\"type\":\"abs\",\"data\":{{\"axis\":\"y\",\"value\":{y}}}}}"
    )
}

/// 通过 QMP 发送一组 `input-send-event` 事件。
fn send_pointer_events(socket: &str, events: &str) -> Result<()> {
    use std::io::{BufRead, BufReader, Write};

    let deadline = Instant::now() + Duration::from_secs(5);
    let stream = loop {
        match TcpStream::connect(socket) {
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

fn inject_codes(socket: &str, codes: &[String]) -> Result<()> {
    use std::io::{BufRead, BufReader, Write};

    let deadline = Instant::now() + Duration::from_secs(5);
    let stream = loop {
        match TcpStream::connect(socket) {
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
    wait_reply(&mut reader)?;

    for key in codes {
        let command = format!(
            "{{\"execute\":\"send-key\",\"arguments\":{{\"keys\":[{{\"type\":\"qcode\",\"data\":\"{key}\"}}]}}}}\n"
        );
        writer
            .write_all(command.as_bytes())
            .and_then(|()| writer.flush())
            .map_err(|error| Failure::new(format!("发送按键失败：{error}")))?;
        wait_reply(&mut reader)?;
        thread::sleep(Duration::from_millis(40));
    }
    Ok(())
}

/// 读取一条 QMP 回执；异步事件行跳过，`error` 直接上抛。
fn wait_reply<R: std::io::BufRead>(reader: &mut R) -> Result<()> {
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|error| Failure::new(format!("读取 QMP 回执失败：{error}")))?;
        if line.trim().is_empty() {
            return Err(Failure::new("QMP 连接提前关闭"));
        }
        if line.contains("\"event\"") {
            continue;
        }
        if line.contains("\"error\"") {
            return Err(Failure::new(format!("QMP 拒绝命令：{}", line.trim())));
        }
        return Ok(());
    }
}

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
