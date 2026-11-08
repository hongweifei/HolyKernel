//! 网络启动自检：ICMP、TCP、UDP 与按需 DNS。

use core::net::Ipv4Addr;

use hk_net::{DnsQueryType, Stack};
use hk_server_api::{exit, write};

use crate::driver::poll_once;
use crate::format::write_address;
use crate::{
    DNS_WAIT_MS, NET_FRAME_MAX, Stats, TCP_PORT, UDP_ECHO_PORT, UDP_LOCAL_PORT, WAIT_MS, now_ms,
};

/// 运行启动自检；失败时以服务退出码结束。
pub(crate) fn run(
    stack: &mut Stack,
    driver: u64,
    endpoint: u64,
    gateway: Ipv4Addr,
    dns_servers: &[Ipv4Addr],
    stats: &mut Stats,
    frame: &mut [u8; NET_FRAME_MAX],
) {
    let icmp = match stack.open_icmp(0x1234) {
        Ok(handle) => handle,
        Err(_) => {
            write("hk-server-net: ICMP 初始化失败\n".as_bytes());
            exit(5)
        }
    };
    let _ = stack.send_ping(icmp, gateway, 0x1234, 1, b"holy");
    let deadline = now_ms().saturating_add(WAIT_MS);
    let mut reply = [0_u8; 64];
    let mut ping_ok = false;
    while now_ms() < deadline {
        poll_once(driver, endpoint, stack, stats, frame);
        if let Ok(Some((length, _))) = stack.recv_ping(icmp, &mut reply)
            && length >= 8
        {
            ping_ok = true;
            break;
        }
    }
    if !ping_ok {
        write("hk-server-net: ping failed\n".as_bytes());
        exit(6)
    }
    write("hk-server-net: ping ok\n".as_bytes());

    let tcp = match stack.open_tcp(2048) {
        Ok(handle) => handle,
        Err(_) => {
            write("hk-server-net: TCP 初始化失败\n".as_bytes());
            exit(7)
        }
    };
    if stack.connect_tcp(tcp, gateway, TCP_PORT, 40000).is_err() {
        write("hk-server-net: TCP 连接失败\n".as_bytes());
        exit(7)
    }
    let connect_deadline = now_ms().saturating_add(WAIT_MS);
    let mut connected = false;
    while now_ms() < connect_deadline {
        poll_once(driver, endpoint, stack, stats, frame);
        if stack.tcp_may_send(tcp) {
            connected = true;
            break;
        }
    }
    if !connected {
        write("hk-server-net: TCP 握手失败\n".as_bytes());
        exit(7)
    }
    let _ = stack.send_tcp(tcp, b"holy");
    let mut reply = [0_u8; 8];
    let mut tcp_ok = false;
    let reply_deadline = now_ms().saturating_add(WAIT_MS);
    while now_ms() < reply_deadline {
        poll_once(driver, endpoint, stack, stats, frame);
        if let Ok(Some(length)) = stack.recv_tcp(tcp, &mut reply)
            && length >= 4
        {
            tcp_ok = true;
            break;
        }
    }
    if !tcp_ok {
        write("hk-server-net: TCP 回显失败\n".as_bytes());
        exit(7)
    }
    write("hk-server-net: tcp ok\n".as_bytes());

    let udp = match stack.open_udp(UDP_LOCAL_PORT) {
        Ok(handle) => handle,
        Err(_) => {
            write("hk-server-net: UDP 初始化失败\n".as_bytes());
            exit(8)
        }
    };
    if stack
        .send_udp(udp, gateway, UDP_ECHO_PORT, b"holy")
        .is_err()
    {
        write("hk-server-net: UDP 发送失败\n".as_bytes());
        exit(8)
    }
    let mut udp_reply = [0_u8; 16];
    let mut udp_ok = false;
    let udp_deadline = now_ms().saturating_add(WAIT_MS);
    while now_ms() < udp_deadline {
        poll_once(driver, endpoint, stack, stats, frame);
        if let Ok(Some((length, _))) = stack.recv_udp(udp, &mut udp_reply)
            && length >= 4
        {
            udp_ok = true;
            break;
        }
    }
    stack.close_socket(udp);
    if !udp_ok {
        write("hk-server-net: UDP 回显失败\n".as_bytes());
        exit(8)
    }
    write("hk-server-net: udp ok\n".as_bytes());

    // DNS 自检按需开启：`net.dns=<name>`。默认启动路径只验证 DHCP/ICMP/TCP/UDP。
    let mut name_buffer = [0_u8; 128];
    if let Some(name) = hk_server_api::boot_cmdline(&mut name_buffer).and_then(|line| {
        line.split_whitespace()
            .find_map(|part| part.strip_prefix("net.dns="))
    }) {
        let dns_server = dns_servers
            .first()
            .copied()
            .unwrap_or(Ipv4Addr::new(10, 0, 2, 3));
        write("hk-server-net: dns-start\n".as_bytes());
        resolve(stack, &[dns_server], name, driver, endpoint, stats, frame);
    }
}

/// 用给定 DNS 服务器解析名称，并把结果写入控制台。
#[cfg(target_os = "none")]
fn resolve(
    stack: &mut Stack,
    servers: &[Ipv4Addr],
    name: &str,
    driver: u64,
    endpoint: u64,
    stats: &mut Stats,
    frame: &mut [u8; NET_FRAME_MAX],
) {
    let Ok(dns) = stack.open_dns(servers) else {
        write("hk-server-net: dns-open-failed\n".as_bytes());
        return;
    };
    let query = match stack.start_dns_query(dns, name, DnsQueryType::A) {
        Ok(query) => query,
        Err(_) => {
            write("hk-server-net: dns-query-failed\n".as_bytes());
            stack.close_socket(dns);
            return;
        }
    };
    let deadline = now_ms().saturating_add(DNS_WAIT_MS);
    while now_ms() < deadline {
        poll_once(driver, endpoint, stack, stats, frame);
        match stack.poll_dns(dns, query) {
            Ok(Some(addresses)) if !addresses.is_empty() => {
                write("hk-server-net: dns ok".as_bytes());
                for address in &addresses {
                    write(b" ");
                    write_address(address);
                }
                write(b"\n");
                stack.close_socket(dns);
                return;
            }
            Ok(Some(_)) => {
                write("hk-server-net: dns empty\n".as_bytes());
                stack.close_socket(dns);
                return;
            }
            Ok(None) => {}
            Err(_) => {
                write("hk-server-net: dns failed\n".as_bytes());
                stack.close_socket(dns);
                return;
            }
        }
    }
    stack.close_socket(dns);
    write("hk-server-net: dns timeout\n".as_bytes());
}
