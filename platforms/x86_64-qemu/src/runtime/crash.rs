//! 最近一次用户异常：现场、栈回溯与符号化。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use hk_platform_x86_64::cpu::{self, InterruptFrame};
use hk_platform_x86_64::paging;
use spin::Mutex;

use super::symbols;

/// 回溯帧数上限。
const MAX_FRAMES: usize = 12;
/// 栈扫描字数上限。
const STACK_WORDS: usize = 48;

struct Record {
    task: String,
    vector: u64,
    error: u64,
    address: u64,
    frames: Vec<u64>,
}

static LAST: Mutex<Option<Record>> = Mutex::new(None);

/// 记录一次用户异常。
pub(super) fn record(frame: &InterruptFrame, task: &str) {
    let mut frames = Vec::new();
    frames.push(frame.rip);

    let mut rbp = frame.rbp;
    for _ in 0..MAX_FRAMES.saturating_sub(1) {
        if !readable(rbp) || !readable(rbp.saturating_add(8)) {
            break;
        }
        let next = read(rbp);
        let ret = read(rbp.saturating_add(8));
        if ret == 0 {
            break;
        }
        push(&mut frames, ret);
        if next <= rbp {
            break;
        }
        rbp = next;
    }

    let mut address = frame.rsp;
    for _ in 0..STACK_WORDS {
        if frames.len() >= MAX_FRAMES || !readable(address) {
            break;
        }
        let word = read(address);
        if symbols::is_code(word) {
            push(&mut frames, word);
        }
        address = address.wrapping_add(8);
    }

    *LAST.lock() = Some(Record {
        task: String::from(task),
        vector: frame.vector,
        error: frame.error,
        address: cpu::fault_address(),
        frames,
    });
}

/// 格式化最近一次异常与符号化回溯。
pub(super) fn format_last() -> String {
    let guard = LAST.lock();
    let Some(record) = guard.as_ref() else {
        return String::from("无异常记录\n");
    };
    let mut text = format!(
        "任务：{}\n异常：{}（向量 {}，错误码 {:#x}）\n",
        record.task,
        cpu::exception_name(record.vector),
        record.vector,
        record.error
    );
    text.push_str(&format!("cr2={:#x}\n", record.address));
    for (index, frame) in record.frames.iter().enumerate() {
        text.push_str(&format!("  #{index} {}\n", symbolize(*frame)));
    }
    text
}

/// 符号化一个地址。
pub(super) fn symbolize(address: u64) -> String {
    match symbols::lookup(address) {
        Some((name, 0)) => format!("{address:#018x} {name}"),
        Some((name, offset)) => format!("{address:#018x} {name}+0x{offset:x}"),
        None => format!("{address:#018x} ?"),
    }
}

/// 符号化以逗号或空白分隔的地址列表。
pub(super) fn symbolize_list(argument: &str) -> String {
    let mut text = String::new();
    for token in argument
        .split(|character: char| character == ',' || character.is_whitespace())
        .filter(|token| !token.is_empty())
    {
        match u64::from_str_radix(token.trim_start_matches("0x"), 16) {
            Ok(address) => {
                text.push_str(&symbolize(address));
                text.push('\n');
            }
            Err(_) => text.push_str("无效地址\n"),
        }
    }
    if text.is_empty() {
        text.push_str("用法：symbolize <十六进制地址>...\n");
    }
    text
}

fn readable(address: u64) -> bool {
    (0x1000..0x0000_0080_0000_0000).contains(&address) && paging::is_user_mapped(address)
}

fn read(address: u64) -> u64 {
    // SAFETY: 调用前已确认地址落在用户态且页表映射存在。
    unsafe { core::ptr::read_unaligned(address as *const u64) }
}

fn push(frames: &mut Vec<u64>, address: u64) {
    if !frames.contains(&address) {
        frames.push(address);
    }
}
