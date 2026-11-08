//! 用户态符号查询与异常回溯格式化。
//!
//! 内核只保存内核符号并输出原始异常现场；服务符号留在各自的 `.hkx` 镜像里，
//! 由用户态调试服务按地址按需查询。回溯过滤与格式化属于用户态策略，不进入
//! 内核常驻状态。

#![no_std]

extern crate alloc;

#[cfg(test)]
extern crate std;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use hk_syscall::FAULT_EVENT;
use hk_syscall_client::root::{ROOT_OP_LOOKUP, call};
use spin::Mutex;

/// 最近一次由内核投递的异常现场。
static LAST: Mutex<Option<String>> = Mutex::new(None);

/// Root 控制缓冲长度：一次批量查询足以容纳数十个符号名。
const CONTROL_BUFFER: usize = 16 * 1024;
/// 单次控制请求的参数上限；内核按 128 字节接收。
const CONTROL_ARGUMENT: usize = 120;
/// 回溯帧数上限。
const MAX_FRAMES: usize = 12;
/// 视为代码的符号偏移上限。
const CODE_WINDOW: u64 = 0x4000;

/// 通过 Root 查询符号；`service` 为空时查内核符号。
pub fn lookup(service: &str, address: u64) -> Option<(String, u64)> {
    lookup_many(service, &[address])
        .into_iter()
        .next()
        .flatten()
}

/// 批量查询符号；返回顺序与输入一致，未命中为 `None`。
pub fn lookup_many(service: &str, addresses: &[u64]) -> Vec<Option<(String, u64)>> {
    let mut result = Vec::with_capacity(addresses.len());
    let mut index = 0;
    while index < addresses.len() {
        let mut argument = String::from(service);
        let mut end = index;
        while end < addresses.len() {
            let mut token = String::from(" ");
            token.push_str(&format!("{:#x}", addresses[end]));
            if argument.len() + token.len() > CONTROL_ARGUMENT {
                break;
            }
            argument.push_str(&token);
            end += 1;
        }
        if end == index {
            argument.push(' ');
            argument.push_str(&format!("{:#x}", addresses[index]));
            end += 1;
        }
        result.extend(query(&argument, end - index));
        index = end;
    }
    result
}

fn query(argument: &str, expected: usize) -> Vec<Option<(String, u64)>> {
    let mut buffer = [0_u8; CONTROL_BUFFER];
    let Ok(length) = call(ROOT_OP_LOOKUP, argument, &mut buffer) else {
        return vec![None; expected];
    };
    let Ok(text) = core::str::from_utf8(&buffer[..length.min(buffer.len())]) else {
        return vec![None; expected];
    };
    let mut result = Vec::with_capacity(expected);
    for line in text.lines() {
        let line = line.trim();
        if line == "none" {
            result.push(None);
            continue;
        }
        let Some((offset, name)) = line.split_once(' ') else {
            result.push(None);
            continue;
        };
        result.push(
            u64::from_str_radix(offset, 16)
                .ok()
                .map(|offset| (String::from(name), offset)),
        );
    }
    result.resize(expected, None);
    result
}

/// 符号化一个地址；无法解析时返回地址本身。
pub fn symbolize(service: &str, address: u64) -> String {
    match lookup(service, address) {
        Some((name, 0)) => format!("{address:#018x} {name}"),
        Some((name, offset)) => format!("{address:#018x} {name}+0x{offset:x}"),
        None => format!("{address:#018x} ?"),
    }
}

/// 接收内核投递的异常事件；供服务事件循环调用。
pub fn ingest(label: u64, data: &[u8]) {
    if label != FAULT_EVENT {
        return;
    }
    let text = String::from_utf8_lossy(data).into_owned();
    *LAST.lock() = Some(text);
}

/// 读取并格式化最近一次用户异常。
pub fn fault_text() -> String {
    let raw = match LAST.lock().as_ref() {
        Some(text) => text.clone(),
        None => return String::from("无异常记录\n"),
    };
    if raw.trim() == "none" {
        return String::from("无异常记录\n");
    }
    let Some(fault) = parse_fault(&raw) else {
        return raw;
    };

    let mut addresses = Vec::new();
    addresses.push(fault.rip);
    for address in fault.backtrace.iter().chain(fault.stack.iter()) {
        if !addresses.contains(address) {
            addresses.push(*address);
        }
    }
    let service = lookup_many(&fault.task, &addresses);
    let kernel = lookup_many("", &addresses);
    let resolve = |address: u64| -> Option<(String, u64)> {
        let index = addresses
            .iter()
            .position(|candidate| *candidate == address)?;
        service
            .get(index)
            .cloned()
            .flatten()
            .or_else(|| kernel.get(index).cloned().flatten())
    };

    let mut frames = vec![fault.rip];
    for address in fault.backtrace.iter().chain(fault.stack.iter()) {
        if frames.len() >= MAX_FRAMES || frames.contains(address) {
            continue;
        }
        if resolve(*address).is_some_and(|(_, offset)| offset < CODE_WINDOW) {
            frames.push(*address);
        }
    }

    let mut text = format!(
        "任务：{}\n异常：{}（向量 {}，错误码 {:#x}）\ncr2={:#x}\n",
        fault.task, fault.kind, fault.vector, fault.error, fault.address
    );
    for (index, frame) in frames.iter().enumerate() {
        let line = match resolve(*frame) {
            Some((name, 0)) => format!("{frame:#018x} {name}"),
            Some((name, offset)) => format!("{frame:#018x} {name}+0x{offset:x}"),
            None => format!("{frame:#018x} ?"),
        };
        text.push_str(&format!("  #{index} {line}\n"));
    }
    text
}

struct Fault {
    task: String,
    kind: String,
    vector: u64,
    error: u64,
    address: u64,
    rip: u64,
    backtrace: Vec<u64>,
    stack: Vec<u64>,
}

fn parse_fault(text: &str) -> Option<Fault> {
    let mut task = None;
    let mut kind = None;
    let mut vector = 0;
    let mut error = 0;
    let mut address = 0;
    let mut rip = 0;
    let mut backtrace = Vec::new();
    let mut stack = Vec::new();
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("task=") {
            task = Some(String::from(value));
        } else if let Some(value) = line.strip_prefix("kind=") {
            kind = Some(String::from(value));
        } else if let Some(value) = line.strip_prefix("vector=") {
            vector = value.trim().parse().ok()?;
        } else if let Some(value) = line.strip_prefix("error=") {
            error = parse_hex(value)?;
        } else if let Some(value) = line.strip_prefix("address=") {
            address = parse_hex(value)?;
        } else if let Some(value) = line.strip_prefix("rip=") {
            rip = parse_hex(value)?;
        } else if let Some(value) = line.strip_prefix("backtrace=") {
            backtrace = parse_list(value);
        } else if let Some(value) = line.strip_prefix("stack=") {
            stack = parse_list(value);
        }
    }
    Some(Fault {
        task: task?,
        kind: kind?,
        vector,
        error,
        address,
        rip,
        backtrace,
        stack,
    })
}

fn parse_hex(text: &str) -> Option<u64> {
    u64::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok()
}

fn parse_list(text: &str) -> Vec<u64> {
    text.split_whitespace().filter_map(parse_hex).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_raw_fault_records() {
        let raw = "task=hk-server-fault\nkind=页错误\nvector=14\nerror=0x4\naddress=0x0\nrip=0x20\nbacktrace=0x20 0x30\nstack=0x40 0x50\n";
        let fault = parse_fault(raw).expect("解析");
        assert_eq!(fault.task, "hk-server-fault");
        assert_eq!(fault.kind, "页错误");
        assert_eq!(fault.vector, 14);
        assert_eq!(fault.error, 4);
        assert_eq!(fault.rip, 0x20);
        assert_eq!(fault.backtrace, [0x20, 0x30]);
        assert_eq!(fault.stack, [0x40, 0x50]);
    }
}
