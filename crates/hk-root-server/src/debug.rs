//! 符号表与用户异常现场格式化。
//!
//! 平台提供符号表字节、异常现场与用户内存访问；本模块负责解析与生成原始
//! 现场文本。异常内容不驻留内核，由订阅者（用户态调试服务）保存与符号化。

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

static SYMBOLS: Mutex<Vec<Symbol>> = Mutex::new(Vec::new());

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

/// 生成一次用户异常的原始现场文本；符号化由用户态调试服务完成。
pub fn format_record<M: Memory>(task: &str, context: FaultContext, memory: &M) -> String {
    let mut backtrace = Vec::new();
    backtrace.push(context.rip);

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
        push(&mut backtrace, ret);
        if next <= rbp {
            break;
        }
        rbp = next;
    }

    let mut stack = Vec::new();
    let mut address = context.rsp;
    for _ in 0..STACK_WORDS {
        if !memory.mapped(address) {
            break;
        }
        let Some(word) = read_u64(memory, address) else {
            break;
        };
        stack.push(word);
        address = address.wrapping_add(8);
    }

    let mut text = format!(
        "task={}\nkind={}\nvector={}\nerror={:#x}\naddress={:#x}\nrip={:#x}\nrbp={:#x}\nrsp={:#x}\n",
        task,
        context.kind,
        context.vector,
        context.error,
        context.address,
        context.rip,
        context.rbp,
        context.rsp
    );
    text.push_str("backtrace=");
    push_hex_list(&mut text, &backtrace);
    text.push('\n');
    text.push_str("stack=");
    push_hex_list(&mut text, &stack);
    text.push('\n');
    text
}

fn push_hex_list(text: &mut String, values: &[u64]) {
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            text.push(' ');
        }
        text.push_str(&format!("{value:#x}"));
    }
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
