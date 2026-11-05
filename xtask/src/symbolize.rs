//! 用内核与服务符号离线解析地址。

use clap::Args;
use hk_image::Image;

use crate::catalog::SERVERS;
use crate::failure::{Failure, Result};
use crate::native::{kernel_symbols, service_image};

#[derive(Debug, Args)]
pub struct SymbolizeArguments {
    /// 地址，支持 0x 前缀
    pub address: String,
    /// 只查询指定服务
    #[arg(long)]
    pub service: Option<String>,
    /// 使用 release 构建产物
    #[arg(long)]
    pub release: bool,
}

pub fn run(arguments: SymbolizeArguments) -> Result<()> {
    let address = parse_address(&arguments.address)?;
    let mut best: Option<(u64, String, String)> = None;

    if arguments.service.is_none() {
        for (symbol, name) in kernel_symbols(arguments.release)? {
            consider(&mut best, address, symbol, "kernel", &name);
        }
    }

    let services: Vec<&str> = arguments
        .service
        .as_deref()
        .map(|service| vec![service])
        .unwrap_or_else(|| SERVERS.to_vec());
    for service in services {
        let (_, bytes) = match service_image(service, arguments.release) {
            Ok(image) => image,
            Err(error) if arguments.service.is_none() => {
                let _ = error;
                continue;
            }
            Err(error) => return Err(error),
        };
        let Some(image) = Image::parse(&bytes) else {
            continue;
        };
        for symbol in image.symbols() {
            consider(&mut best, address, symbol.address, service, symbol.name);
        }
    }

    let Some((symbol, module, name)) = best else {
        return Err(Failure::new(format!("未找到覆盖 {address:#018x} 的符号")));
    };
    let offset = address - symbol;
    let name = name.strip_prefix(&format!("[{module}] ")).unwrap_or(&name);
    if offset == 0 {
        println!("{address:#018x} [{module}] {name}");
    } else {
        println!("{address:#018x} [{module}] {name}+0x{offset:x}");
    }
    Ok(())
}

fn consider(
    best: &mut Option<(u64, String, String)>,
    address: u64,
    symbol: u64,
    module: &str,
    name: &str,
) {
    if symbol > address {
        return;
    }
    if best
        .as_ref()
        .is_none_or(|(current, _, _)| symbol > *current)
    {
        *best = Some((symbol, module.to_string(), name.to_string()));
    }
}

fn parse_address(text: &str) -> Result<u64> {
    let text = text.trim();
    let digits = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .unwrap_or(text);
    u64::from_str_radix(digits, 16).map_err(|_| {
        Failure::new(format!(
            "地址无效：{text}；请使用十六进制，例如 0xffffffff80018bc4"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_prefixed_and_bare_addresses() {
        assert_eq!(parse_address("0x1000").ok(), Some(0x1000));
        assert_eq!(parse_address("1000").ok(), Some(0x1000));
        assert!(parse_address("xyz").is_err());
    }

    #[test]
    fn picks_nearest_symbol_at_or_below_address() {
        let mut best = None;
        consider(&mut best, 0x20, 0x10, "demo", "a");
        consider(&mut best, 0x20, 0x18, "demo", "b");
        consider(&mut best, 0x20, 0x30, "demo", "c");
        assert_eq!(best, Some((0x18, "demo".into(), "b".into())));
    }
}
