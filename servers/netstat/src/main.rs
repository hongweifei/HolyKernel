//! netstat：向 Network Server 查询接口、Socket 与收发统计并打印。
//!
//! 作为独立用户态服务运行，只依赖 `hk-server-api` 的线协议；不把网络实现引入
//! 内核或 Shell。`net` 命令通过运行时按需启动本服务。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;
#[cfg(target_os = "none")]
use hk_server_api::net::{
    ArpTable, NET_ARP_BYTES, NET_ARP_REQUEST, NET_ARP_RESPONSE, NET_STATUS_BYTES,
    NET_STATUS_REQUEST, NET_STATUS_RESPONSE, NetStatus,
};
#[cfg(target_os = "none")]
use hk_server_api::{exit, recv_data_from, send, service_endpoint, write, write_u64, yield_now};

/// 等待 Network Server 出现的重试次数。
#[cfg(target_os = "none")]
const WAIT_ATTEMPTS: usize = 4096;

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let net = wait_net();
    if net == 0 {
        write(b"hk-server-netstat: network server not found\n");
        exit(1)
    }
    if send(net, NET_STATUS_REQUEST, 0) != 0 {
        write(b"hk-server-netstat: status request failed\n");
        exit(2)
    }
    let mut buffer = [0_u8; NET_STATUS_BYTES];
    let (label, _word, sender, length) = recv_data_from(endpoint, net, &mut buffer);
    if label != NET_STATUS_RESPONSE || sender != net || length < NET_STATUS_BYTES {
        write(b"hk-server-netstat: bad response\n");
        exit(3)
    }
    let Some(status) = NetStatus::decode(&buffer[..length]) else {
        write(b"hk-server-netstat: decode failed\n");
        exit(3)
    };
    print_status(&status);
    print_routes(&status);
    print_arp(endpoint, net);
    exit(0)
}

/// 打印由接口地址与默认网关合成的路由视图。
#[cfg(target_os = "none")]
fn print_routes(status: &NetStatus) {
    if status.has_ipv4 {
        write(b"hk-server-netstat: route=");
        write_ipv4(&status.ipv4);
        write(b"/");
        write_u64(u64::from(status.prefix));
        write(b" dev=eth0\n");
    }
    if status.has_gateway {
        write(b"hk-server-netstat: route=0.0.0.0/0 via=");
        write_ipv4(&status.gateway);
        write(b"\n");
    }
}

/// 查询并打印 ARP 邻居表。
#[cfg(target_os = "none")]
fn print_arp(endpoint: u64, net: u64) {
    if send(net, NET_ARP_REQUEST, 0) != 0 {
        write(b"hk-server-netstat: arp request failed\n");
        return;
    }
    let mut buffer = [0_u8; NET_ARP_BYTES];
    let (label, _word, sender, length) = recv_data_from(endpoint, net, &mut buffer);
    if label != NET_ARP_RESPONSE || sender != net {
        write(b"hk-server-netstat: arp bad response\n");
        return;
    }
    let Some(table) = ArpTable::decode(&buffer[..length]) else {
        write(b"hk-server-netstat: arp decode failed\n");
        return;
    };
    if table.count == 0 {
        write(b"hk-server-netstat: arp=none\n");
        return;
    }
    for entry in &table.entries[..usize::from(table.count)] {
        write(b"hk-server-netstat: arp=");
        write_ipv4(&entry.ip);
        write(b" mac=");
        for (index, byte) in entry.mac.iter().enumerate() {
            if index != 0 {
                write(b":");
            }
            write_hex(*byte);
        }
        write(b"\n");
    }
}

#[cfg(target_os = "none")]
fn print_status(status: &NetStatus) {
    write(b"hk-server-netstat: mac=");
    for (index, byte) in status.mac.iter().enumerate() {
        if index != 0 {
            write(b":");
        }
        write_hex(*byte);
    }
    write(b" ipv4=");
    if status.has_ipv4 {
        write_ipv4(&status.ipv4);
        write(b"/");
        write_u64(u64::from(status.prefix));
    } else {
        write(b"none");
    }
    write(b" gw=");
    if status.has_gateway {
        write_ipv4(&status.gateway);
    } else {
        write(b"none");
    }
    write(b" sockets=");
    write_u64(u64::from(status.sockets));
    write(b"\n");

    write(b"hk-server-netstat: rx_frames=");
    write_u64(status.rx_frames);
    write(b" tx_frames=");
    write_u64(status.tx_frames);
    write(b" rx_bytes=");
    write_u64(status.rx_bytes);
    write(b" tx_bytes=");
    write_u64(status.tx_bytes);
    write(b"\n");
}

#[cfg(target_os = "none")]
fn write_ipv4(address: &[u8; 4]) {
    for (index, octet) in address.iter().enumerate() {
        if index != 0 {
            write(b".");
        }
        write_u64(u64::from(*octet));
    }
}

#[cfg(target_os = "none")]
fn write_hex(byte: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let buffer = [HEX[usize::from(byte >> 4)], HEX[usize::from(byte & 0xF)]];
    write(&buffer);
}

#[cfg(target_os = "none")]
fn wait_net() -> u64 {
    for _ in 0..WAIT_ATTEMPTS {
        let endpoint = service_endpoint("hk-server-net");
        if endpoint != 0 {
            return endpoint;
        }
        let _ = yield_now();
    }
    0
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(not(target_os = "none"))]
fn main() {
    eprintln!("hk-server-netstat 只在 x86_64-unknown-none 目标上构建");
}
