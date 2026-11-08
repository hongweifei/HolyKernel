//! Network Server：smoltcp 协议栈与 virtio-net Endpoint 之间的桥接。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
#[cfg(target_os = "none")]
use core::net::Ipv4Addr;

#[cfg(target_os = "none")]
use hk_net::Stack;
#[cfg(target_os = "none")]
use hk_proto::net::{
    ArpRecord, ArpTable, NET_ARP_BYTES, NET_ARP_MAX, NET_ARP_REQUEST, NET_ARP_RESPONSE,
    NET_CONFIG_BYTES, NET_CONFIG_MAX_DNS, NET_CONFIG_REQUEST, NET_CONFIG_RESPONSE, NET_FRAME_MAX,
    NET_INFO_BYTES, NET_INFO_REQUEST, NET_INFO_RESPONSE, NET_NOTIFY_TX, NET_POLL_DONE, NET_RX,
    NET_RX_FRAME, NET_STATUS_BYTES, NET_STATUS_REQUEST, NET_STATUS_RESPONSE, NET_TX_DONE,
    NetConfig, net_rx_index, net_rx_length, net_rx_offset, net_tx_index,
};
#[cfg(target_os = "none")]
use hk_proto::socket::{
    SOCK_CLOSE, SOCK_CONNECT, SOCK_OPEN, SOCK_RECV, SOCK_RESOLVE, SOCK_SEND, SOCK_UDP_RECV,
    SOCK_UDP_SEND,
};
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::{
    exit, notify, recv_data, recv_data_handle, send, send_data, tsc, tsc_frequency, write,
};

#[cfg(target_os = "none")]
mod config;
#[cfg(target_os = "none")]
mod driver;
#[cfg(target_os = "none")]
mod format;
#[cfg(target_os = "none")]
mod protocol;
#[cfg(target_os = "none")]
mod selftest;

#[cfg(target_os = "none")]
use config::{parse_resolvers, parse_static, zerocopy_enabled};
#[cfg(target_os = "none")]
use driver::{attach_shared, poll_once, restart_network, wait_driver};
#[cfg(target_os = "none")]
#[cfg(target_os = "none")]
use protocol::{flush_tx, handle_socket, status_blob};

#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 768 * 1024;
#[cfg(target_os = "none")]
pub(crate) const TCP_PORT: u16 = 45678;
/// 宿主 UDP echo 端口。
#[cfg(target_os = "none")]
pub(crate) const UDP_ECHO_PORT: u16 = 45679;
/// UDP 自检使用的本地端口。
#[cfg(target_os = "none")]
pub(crate) const UDP_LOCAL_PORT: u16 = 40001;
/// 对外报告的接口 MTU。
#[cfg(target_os = "none")]
const MTU: u16 = 1500;
#[cfg(target_os = "none")]
pub(crate) const WAIT_MS: u64 = 5_000;
#[cfg(target_os = "none")]
pub(crate) const DNS_WAIT_MS: u64 = 10_000;
#[cfg(target_os = "none")]
const POLL_WAIT_MS: u64 = 100;

/// 收发计数。
#[cfg(target_os = "none")]
#[derive(Default)]
pub(crate) struct Stats {
    rx_frames: u64,
    tx_frames: u64,
    rx_bytes: u64,
    tx_bytes: u64,
}

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let mut driver = wait_driver();
    if driver == 0 {
        write("hk-server-net: 没有 virtio-net 驱动\n".as_bytes());
        exit(2)
    }

    let mut info = [0_u8; NET_INFO_BYTES];
    let _ = send(driver, NET_INFO_REQUEST, 0);
    let _ = notify(driver, NET_NOTIFY_TX);
    let (label, _word, _sender, length, _handle) = recv_data_handle(endpoint, &mut info, driver);
    if label != NET_INFO_RESPONSE || length < NET_INFO_BYTES {
        write("hk-server-net: 接口信息失败\n".as_bytes());
        exit(3)
    }
    let mut mac = [0_u8; 6];
    mac.copy_from_slice(&info[..6]);
    let (mut stack, dhcp) = match Stack::new_dhcp(mac, now_ms()) {
        Ok(stack) => stack,
        Err(_) => {
            write("hk-server-net: 协议栈初始化失败\n".as_bytes());
            exit(4)
        }
    };
    if zerocopy_enabled() && !attach_shared(driver, endpoint, &mut stack) {
        write("hk-server-net: 共享帧附加失败\n".as_bytes());
        exit(3)
    }
    let mut frame = [0_u8; NET_FRAME_MAX];
    let mut stats = Stats::default();
    let mut gateway = Ipv4Addr::new(10, 0, 2, 2);
    let mut leased = false;

    // 原生启动期配置：`net.dhcp=off`/`net.ip=`/`net.prefix=`/`net.gw=`/
    // `net.resolver=`。这是 Holy 的引导配置，不读取任何 Unix 文件。
    let mut cmdline_buffer = [0_u8; 256];
    let cmdline = hk_server_api::boot_cmdline(&mut cmdline_buffer);
    let resolvers = cmdline.map(parse_resolvers).unwrap_or_default();
    let static_config = cmdline.and_then(parse_static);
    let mut dns_servers = resolvers;
    if let Some((address, prefix, route)) = static_config {
        if stack.set_ipv4(address, prefix, route).is_err() {
            write("hk-server-net: 静态地址失败\n".as_bytes());
            exit(4)
        }
        if let Some(route) = route {
            gateway = route;
        }
        write("hk-server-net: static ok\n".as_bytes());
    } else {
        let deadline = now_ms().saturating_add(WAIT_MS);
        while now_ms() < deadline {
            poll_once(driver, endpoint, &mut stack, &mut stats, &mut frame);
            if let Some(lease) = stack.poll_dhcp(dhcp) {
                gateway = lease.gateway.unwrap_or(gateway);
                if !lease.dns.is_empty() {
                    dns_servers = lease.dns;
                }
                write("hk-server-net: dhcp ok\n".as_bytes());
                leased = true;
                break;
            }
        }
        if !leased {
            if stack
                .set_ipv4(
                    Ipv4Addr::new(10, 0, 2, 15),
                    24,
                    Some(Ipv4Addr::new(10, 0, 2, 2)),
                )
                .is_err()
            {
                write("hk-server-net: 静态地址失败\n".as_bytes());
                exit(4)
            }
            write("hk-server-net: dhcp timeout, static\n".as_bytes());
        }
    }
    write("hk-server-net: ready\n".as_bytes());
    if !cmdline.is_some_and(|line| line.contains("net.no-selftest")) {
        selftest::run(
            &mut stack,
            driver,
            endpoint,
            gateway,
            &dns_servers,
            &mut stats,
            &mut frame,
        );
    }

    if cmdline.is_some_and(|line| line.contains("net.restart")) {
        driver = restart_network(
            driver, endpoint, &mut stack, &mut stats, &mut frame, gateway,
        );
    }

    write("hk-server-net: serving\n".as_bytes());
    let mut control = [0_u8; 2048];
    let mut next_udp = 40200_u16;
    loop {
        let (label, word, sender, length) = recv_data(endpoint, &mut control);
        match label {
            NET_RX => {
                if sender != 0 && sender != driver {
                    // 驱动重启后端点编号变化，自动切换到新端点。
                    driver = sender;
                }
                let length = length.min(control.len());
                let _ = stack.push_frame(&control[..length]);
                stats.rx_frames = stats.rx_frames.saturating_add(1);
                stats.rx_bytes = stats.rx_bytes.saturating_add(length as u64);
                stack.poll(now_ms());
                flush_tx(driver, &mut stack, &mut stats);
            }
            NET_RX_FRAME => {
                let length = net_rx_length(word);
                stats.rx_frames = stats.rx_frames.saturating_add(1);
                stats.rx_bytes = stats.rx_bytes.saturating_add(length as u64);
                stack.poll(now_ms());
                stack.push_shared_rx(net_rx_index(word), net_rx_offset(word), length);
                stack.poll(now_ms());
                flush_tx(driver, &mut stack, &mut stats);
            }
            NET_TX_DONE => {
                if sender != 0 && sender != driver {
                    driver = sender;
                }
                stack.shared_tx_done(net_tx_index(word));
            }
            NET_POLL_DONE => {}
            SOCK_OPEN | SOCK_CONNECT | SOCK_SEND | SOCK_RECV | SOCK_CLOSE | SOCK_UDP_SEND
            | SOCK_UDP_RECV | SOCK_RESOLVE => {
                handle_socket(
                    label,
                    &control[..length.min(control.len())],
                    sender,
                    driver,
                    endpoint,
                    &mut stack,
                    &mut stats,
                    &dns_servers,
                    &mut frame,
                    &mut next_udp,
                );
            }
            NET_STATUS_REQUEST => {
                let mut encoded = [0_u8; NET_STATUS_BYTES];
                if let Some(length) = status_blob(&stack, &stats, &mut encoded) {
                    let _ = send_data(sender, NET_STATUS_RESPONSE, &encoded[..length]);
                }
            }
            NET_ARP_REQUEST => {
                let mut table = ArpTable::EMPTY;
                for (index, entry) in stack.arp_entries().iter().take(NET_ARP_MAX).enumerate() {
                    table.entries[index] = ArpRecord {
                        ip: entry.ip.octets(),
                        mac: entry.mac.0,
                    };
                    table.count += 1;
                }
                let mut encoded = [0_u8; NET_ARP_BYTES];
                if let Some(length) = table.encode(&mut encoded) {
                    let _ = send_data(sender, NET_ARP_RESPONSE, &encoded[..length]);
                }
            }
            NET_CONFIG_REQUEST => {
                let mut config = NetConfig {
                    dhcp: leased,
                    mtu: MTU,
                    ..NetConfig::default()
                };
                let status = stack.status();
                if let Some(address) = status.ipv4 {
                    config.ipv4 = address.octets();
                    config.has_ipv4 = true;
                    config.prefix = status.prefix;
                }
                if let Some(gateway) = status.gateway {
                    config.gateway = gateway.octets();
                    config.has_gateway = true;
                }
                for (index, server) in dns_servers.iter().take(NET_CONFIG_MAX_DNS).enumerate() {
                    config.dns[index] = server.octets();
                    config.dns_count += 1;
                }
                let mut encoded = [0_u8; NET_CONFIG_BYTES];
                if let Some(length) = config.encode(&mut encoded) {
                    let _ = send_data(sender, NET_CONFIG_RESPONSE, &encoded[..length]);
                }
            }
            _ => {}
        }
    }
}

/// 以 TSC 换算单调毫秒时间；TSC 未知时退回累计 tick（约 10 ms）。
#[cfg(target_os = "none")]
pub(crate) fn now_ms() -> u64 {
    let frequency = tsc_frequency();
    if frequency == 0 {
        return hk_server_api::ticks().saturating_mul(10);
    }
    tsc().saturating_mul(1_000) / frequency
}

hk_server_api::server_boilerplate!();
