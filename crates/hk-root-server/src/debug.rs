//! 符号表与用户异常回溯。
//!
//! 平台提供符号表字节、异常现场与用户内存访问；本模块负责解析、记录与
//! 格式化，保持调试能力与具体架构无关。

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use spin::Mutex;

/// 回溯帧数上限。
const MAX_FRAMES: usize = 12;
/// 栈扫描字数上限。
const STACK_WORDS: usize = 48;

struct Symbol {
    address: u64,
    name: &'static str,
}

struct Record {
    task: String,
    kind: &'static str,
    vector: u64,
    error: u64,
    address: u64,
    frames: Vec<u64>,
}

static SYMBOLS: Mutex<Vec<Symbol>> = Mutex::new(Vec::new());
static LAST: Mutex<Option<Record>> = Mutex::new(None);

/// 异常现场。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaultContext {
    /// 异常名称。
    pub kind: &'static str,
    /// 中断向量。
    pub vector: u64,
    /// 错误码。
    pub error: u64,
    /// 访问地址（如页错误地址）。
    pub address: u64,
    /// 出错指令地址。
    pub rip: u64,
    /// 栈指针。
    pub rsp: u64,
    /// 帧指针；为 0 时跳过帧链。
    pub rbp: u64,
}

/// 用户内存访问。
pub trait Memory {
    /// 判断地址是否为用户可读页。
    fn mapped(&self, address: u64) -> bool;
    /// 读取用户内存；失败返回 `false`。
    fn read(&self, address: u64, buffer: &mut [u8]) -> bool;
}

/// 安装符号表；每行格式为 `地址 名称`。
pub fn install_symbols(bytes: &[u8]) {
    let Ok(text) = core::str::from_utf8(bytes) else {
        return;
    };
    let mut symbols = Vec::new();
    for line in text.lines() {
        let Some((address, name)) = parse(line) else {
            continue;
        };
        let name: &'static str = Box::leak(String::from(name).into_boxed_str());
        symbols.push(Symbol { address, name });
    }
    symbols.sort_by_key(|symbol| symbol.address);
    *SYMBOLS.lock() = symbols;
}

/// 把地址解析为 `名字+偏移`。
pub fn lookup(address: u64) -> Option<(&'static str, u64)> {
    let symbols = SYMBOLS.lock();
    let index = symbols.partition_point(|symbol| symbol.address <= address);
    if index == 0 {
        return None;
    }
    let symbol = &symbols[index - 1];
    Some((symbol.name, address - symbol.address))
}

/// 判断地址是否落在某个符号的起始附近。
pub fn is_code(address: u64) -> bool {
    lookup(address).is_some_and(|(_, offset)| offset < 0x4000)
}

/// 符号化一个地址。
pub fn symbolize(address: u64) -> String {
    match lookup(address) {
        Some((name, 0)) => format!("{address:#018x} {name}"),
        Some((name, offset)) => format!("{address:#018x} {name}+0x{offset:x}"),
        None => format!("{address:#018x} ?"),
    }
}

/// 符号化以逗号或空白分隔的地址列表。
pub fn symbolize_list(argument: &str) -> String {
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

/// 记录一次用户异常。
pub fn record<M: Memory>(task: &str, context: FaultContext, memory: &M) {
    let mut frames = Vec::new();
    frames.push(context.rip);

    let mut rbp = context.rbp;
    for _ in 0..MAX_FRAMES.saturating_sub(1) {
        if !memory.mapped(rbp) || !memory.mapped(rbp.saturating_add(8)) {
            break;
        }
        let Some(next) = read_u64(memory, rbp) else {
            break;
        };
        let Some(ret) = read_u64(memory, rbp.saturating_add(8)) else {
            break;
        };
        if ret == 0 {
            break;
        }
        push(&mut frames, ret);
        if next <= rbp {
            break;
        }
        rbp = next;
    }

    let mut address = context.rsp;
    for _ in 0..STACK_WORDS {
        if frames.len() >= MAX_FRAMES || !memory.mapped(address) {
            break;
        }
        let Some(word) = read_u64(memory, address) else {
            break;
        };
        if is_code(word) {
            push(&mut frames, word);
        }
        address = address.wrapping_add(8);
    }

    *LAST.lock() = Some(Record {
        task: String::from(task),
        kind: context.kind,
        vector: context.vector,
        error: context.error,
        address: context.address,
        frames,
    });
}

/// 格式化最近一次异常与符号化回溯。
pub fn last_text() -> String {
    let guard = LAST.lock();
    let Some(record) = guard.as_ref() else {
        return String::from("无异常记录\n");
    };
    let mut text = format!(
        "任务：{}\n异常：{}（向量 {}，错误码 {:#x}）\n",
        record.task, record.kind, record.vector, record.error
    );
    text.push_str(&format!("cr2={:#x}\n", record.address));
    for (index, frame) in record.frames.iter().enumerate() {
        text.push_str(&format!("  #{index} {}\n", symbolize(*frame)));
    }
    text
}

fn read_u64<M: Memory>(memory: &M, address: u64) -> Option<u64> {
    let mut bytes = [0_u8; 8];
    memory
        .read(address, &mut bytes)
        .then(|| u64::from_le_bytes(bytes))
}

fn push(frames: &mut Vec<u64>, address: u64) {
    if !frames.contains(&address) {
        frames.push(address);
    }
}

fn parse(line: &str) -> Option<(u64, &str)> {
    let line = line.trim();
    let address_end = line.find(char::is_whitespace)?;
    let address = u64::from_str_radix(&line[..address_end], 16).ok()?;
    let name = line[address_end..].trim_start();
    (!name.is_empty()).then_some((address, name))
}
