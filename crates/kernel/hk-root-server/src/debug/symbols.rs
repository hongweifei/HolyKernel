//! 内核符号表：安装构建期文本符号并按地址查询。

use alloc::string::String;

use hk_symbols::SymbolTable;
use spin::Mutex;

static SYMBOLS: Mutex<SymbolTable> = Mutex::new(SymbolTable::new());

/// 安装符号表；每行格式为 `地址 名称`。
pub fn install_symbols(bytes: &[u8]) {
    *SYMBOLS.lock() = SymbolTable::from_text(bytes);
}

/// 把地址解析为 `名字+偏移`。
pub fn lookup(address: u64) -> Option<(&'static str, u64)> {
    SYMBOLS.lock().lookup(address)
}

/// 符号化一个地址。
pub fn symbolize(address: u64) -> String {
    SYMBOLS.lock().symbolize(address)
}

/// 符号化以逗号或空白分隔的地址列表。
pub fn symbolize_list(argument: &str) -> String {
    SYMBOLS.lock().symbolize_list(argument)
}
