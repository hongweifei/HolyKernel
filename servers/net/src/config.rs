//! 启动期网络配置解析。

use alloc::vec::Vec;
use core::net::Ipv4Addr;

use hk_server_api::net::NET_CONFIG_MAX_DNS;

/// 是否启用共享帧路径。
#[cfg(target_os = "none")]
pub(crate) fn zerocopy_enabled() -> bool {
    let mut buffer = [0_u8; 128];
    hk_server_api::boot_cmdline(&mut buffer).is_some_and(|line| line.contains("net.zerocopy"))
}

/// 解析原生启动期网络配置；`net.dhcp=off` 或 `net.ip=` 存在时返回静态配置。
#[cfg(target_os = "none")]
pub(crate) fn parse_static(line: &str) -> Option<(Ipv4Addr, u8, Option<Ipv4Addr>)> {
    let mut address = None;
    let mut prefix = 24_u8;
    let mut gateway = None;
    let mut dhcp_off = false;
    for part in line.split_whitespace() {
        if let Some(value) = part.strip_prefix("net.ip=") {
            address = parse_ipv4(value);
        } else if let Some(value) = part.strip_prefix("net.prefix=") {
            if let Ok(value) = value.parse() {
                prefix = value;
            }
        } else if let Some(value) = part.strip_prefix("net.gw=") {
            gateway = parse_ipv4(value);
        } else if part == "net.dhcp=off" {
            dhcp_off = true;
        }
    }
    if address.is_none() && !dhcp_off {
        return None;
    }
    Some((
        address.unwrap_or(Ipv4Addr::new(10, 0, 2, 15)),
        prefix,
        gateway.or(Some(Ipv4Addr::new(10, 0, 2, 2))),
    ))
}

/// 解析 `net.resolver=` 列表（逗号分隔，最多 4 个）。
#[cfg(target_os = "none")]
pub(crate) fn parse_resolvers(line: &str) -> Vec<Ipv4Addr> {
    let mut servers = Vec::new();
    for part in line.split_whitespace() {
        let Some(value) = part.strip_prefix("net.resolver=") else {
            continue;
        };
        for item in value.split(',') {
            if servers.len() >= NET_CONFIG_MAX_DNS {
                return servers;
            }
            if let Some(address) = parse_ipv4(item) {
                servers.push(address);
            }
        }
    }
    servers
}

#[cfg(target_os = "none")]
fn parse_ipv4(text: &str) -> Option<Ipv4Addr> {
    let mut octets = [0_u8; 4];
    let mut index = 0;
    for part in text.split('.') {
        if index == 4 {
            return None;
        }
        octets[index] = part.parse().ok()?;
        index += 1;
    }
    (index == 4).then(|| Ipv4Addr::from(octets))
}
