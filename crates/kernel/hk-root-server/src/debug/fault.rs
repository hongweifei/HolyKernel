//! 用户异常现场：捕获原始寄存器与栈，生成文本记录。
//!
//! 内核只输出原始现场，符号化由用户态调试服务按地址查询。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// 回溯帧数上限。
const MAX_FRAMES: usize = 12;
/// 栈扫描字数上限。
const STACK_WORDS: usize = 48;

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
