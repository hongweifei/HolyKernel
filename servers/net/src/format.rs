//! Network Server 的地址输出格式。

use hk_net::IpAddress;
use hk_server_api::{write, write_u64};

/// 输出 IPv4/IPv6 地址。
#[cfg(target_os = "none")]
pub(crate) fn write_address(address: &IpAddress) {
    match address {
        IpAddress::Ipv4(address) => {
            let octets = address.octets();
            for (index, octet) in octets.iter().enumerate() {
                if index != 0 {
                    write(b".");
                }
                write_u64(u64::from(*octet));
            }
        }
        IpAddress::Ipv6(address) => {
            let octets = address.octets();
            for (index, chunk) in octets.chunks(2).enumerate() {
                if index != 0 {
                    write(b":");
                }
                write_hex(u16::from_be_bytes([chunk[0], chunk[1]]));
            }
        }
    }
}

/// 以十六进制输出 16 位整数。
#[cfg(target_os = "none")]
fn write_hex(value: u16) {
    hk_server_api::format::write_hex(u64::from(value), 4);
}
