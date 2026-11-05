//! 镜像符号表：读取 `kernel.sym` 模块并提供地址解析。

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use hk_hal::BootInfo;
use hk_root_server::loader::module_name;
use spin::Mutex;

/// 镜像内的符号表模块名。
const SYMBOL_MODULE: &str = "kernel.sym";

struct Symbol {
    address: u64,
    name: &'static str,
}

static SYMBOLS: Mutex<Vec<Symbol>> = Mutex::new(Vec::new());

/// 解析镜像内的符号表模块。
pub fn init(info: &BootInfo) {
    let Some(module) = info
        .server_modules
        .iter()
        .find(|module| module_name(module.name) == SYMBOL_MODULE)
    else {
        return;
    };
    let Some(bytes) = info.module_bytes(module) else {
        return;
    };
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

/// 把地址解析为 `名字+偏移`；没有匹配时返回 `None`。
pub fn lookup(address: u64) -> Option<(&'static str, u64)> {
    let symbols = SYMBOLS.lock();
    let index = symbols.partition_point(|symbol| symbol.address <= address);
    if index == 0 {
        return None;
    }
    let symbol = &symbols[index - 1];
    Some((symbol.name, address - symbol.address))
}

/// 判断地址是否落在某个符号的起始处（用于过滤栈上的数据字）。
pub fn is_code(address: u64) -> bool {
    lookup(address).is_some_and(|(_, offset)| offset < 0x4000)
}

fn parse(line: &str) -> Option<(u64, &str)> {
    let line = line.trim();
    let address_end = line.find(char::is_whitespace)?;
    let address = u64::from_str_radix(&line[..address_end], 16).ok()?;
    let name = line[address_end..].trim_start();
    (!name.is_empty()).then_some((address, name))
}
