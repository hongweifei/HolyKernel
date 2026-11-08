//! 文本符号表：解析 `地址 名称`，按地址查找最近符号并格式化。
//!
//! 符号表由构建期从内核镜像提取，启动时一次性安装；名字在安装时驻留，
//! 查询与格式化不再分配符号名。内核与用户态工具共用同一套解析和格式化规则。

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

struct Symbol {
    address: u64,
    name: &'static str,
}

/// 按地址排序的文本符号表。
pub struct SymbolTable {
    symbols: Vec<Symbol>,
}

impl SymbolTable {
    /// 构造空符号表。
    pub const fn new() -> Self {
        Self {
            symbols: Vec::new(),
        }
    }

    /// 解析 `地址 名称` 文本；无效行跳过，名字驻留到进程结束。
    pub fn from_text(bytes: &[u8]) -> Self {
        let Ok(text) = core::str::from_utf8(bytes) else {
            return Self::new();
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
        Self { symbols }
    }

    /// 返回地址之前最近的符号与偏移。
    pub fn lookup(&self, address: u64) -> Option<(&'static str, u64)> {
        let index = self
            .symbols
            .partition_point(|symbol| symbol.address <= address);
        if index == 0 {
            return None;
        }
        let symbol = &self.symbols[index - 1];
        Some((symbol.name, address - symbol.address))
    }

    /// 符号化一个地址。
    pub fn symbolize(&self, address: u64) -> String {
        match self.lookup(address) {
            Some((name, 0)) => format!("{address:#018x} {name}"),
            Some((name, offset)) => format!("{address:#018x} {name}+0x{offset:x}"),
            None => format!("{address:#018x} ?"),
        }
    }

    /// 符号化以逗号或空白分隔的地址列表。
    pub fn symbolize_list(&self, argument: &str) -> String {
        let mut text = String::new();
        for token in argument
            .split(|character: char| character == ',' || character.is_whitespace())
            .filter(|token| !token.is_empty())
        {
            match u64::from_str_radix(token.trim_start_matches("0x"), 16) {
                Ok(address) => {
                    text.push_str(&self.symbolize(address));
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

    /// 返回符号数量。
    pub fn len(&self) -> usize {
        self.symbols.len()
    }

    /// 判断符号表是否为空。
    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty()
    }
}

impl Default for SymbolTable {
    fn default() -> Self {
        Self::new()
    }
}

fn parse(line: &str) -> Option<(u64, &str)> {
    let line = line.trim();
    let address_end = line.find(char::is_whitespace)?;
    let address = u64::from_str_radix(&line[..address_end], 16).ok()?;
    let name = line[address_end..].trim_start();
    (!name.is_empty()).then_some((address, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> SymbolTable {
        SymbolTable::from_text(b"1000 alpha\n2000 beta\n\nzz top\n")
    }

    #[test]
    fn parses_and_sorts_symbols() {
        let table = table();
        assert_eq!(table.len(), 2);
        assert_eq!(table.lookup(0x1000), Some(("alpha", 0)));
        assert_eq!(table.lookup(0x1FFF), Some(("alpha", 0xFFF)));
        assert_eq!(table.lookup(0x2004), Some(("beta", 4)));
        assert_eq!(table.lookup(0x0FFF), None);
    }

    #[test]
    fn formats_addresses() {
        let table = table();
        assert_eq!(table.symbolize(0x2000), "0x0000000000002000 beta");
        assert_eq!(table.symbolize(0x2004), "0x0000000000002004 beta+0x4");
        assert_eq!(table.symbolize(0x0500), "0x0000000000000500 ?");
    }

    #[test]
    fn formats_address_lists() {
        let table = table();
        let text = table.symbolize_list("0x2000, 0x0500");
        assert!(text.contains("beta"));
        assert!(text.contains('?'));
        assert_eq!(
            table.symbolize_list(""),
            "用法：symbolize <十六进制地址>...\n"
        );
    }

    #[test]
    fn empty_or_invalid_input_is_empty() {
        let table = SymbolTable::from_text(&[0xFF, 0xFE]);
        assert!(table.is_empty());
        assert_eq!(SymbolTable::new().lookup(1), None);
    }
}
