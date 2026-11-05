//! Network Server：smoltcp 协议栈与 virtio-net Endpoint 之间的桥接。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::vec::Vec;

#[cfg(target_os = "none")]
use core::net::Ipv4Addr;

#[cfg(target_os = "none")]
use hk_net::{DnsQueryType, IpAddress, SocketId, Stack};
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::net::{
    ArpRecord, ArpTable, NET_ARP_BYTES, NET_ARP_MAX, NET_ARP_REQUEST, NET_ARP_RESPONSE,
    NET_CONFIG_BYTES, NET_CONFIG_MAX_DNS, NET_CONFIG_REQUEST, NET_CONFIG_RESPONSE,
    NET_FRAME_ATTACH, NET_FRAME_MAX, NET_FRAME_ROLE_RX, NET_FRAME_ROLE_TX, NET_INFO_BYTES,
    NET_INFO_REQUEST, NET_INFO_RESPONSE, NET_NOTIFY_TX, NET_POLL, NET_POLL_DONE, NET_RX,
    NET_RX_FRAME, NET_RX_REFILL, NET_SHARED_FRAME, NET_SHARED_RX, NET_SHARED_TX, NET_SHUTDOWN,
    NET_STATUS_BYTES, NET_STATUS_REQUEST, NET_STATUS_RESPONSE, NET_TX, NET_TX_DONE, NET_TX_FRAME,
    NetConfig, NetStatus, net_frame_index, net_frame_role, net_rx_index, net_rx_length,
    net_rx_offset, net_tx_index, net_tx_word,
};
#[cfg(target_os = "none")]
use hk_server_api::socket::{
    SOCK_CLOSE, SOCK_CONNECT, SOCK_ERROR, SOCK_KIND_TCP, SOCK_KIND_UDP, SOCK_OPEN,
    SOCK_PAYLOAD_MAX, SOCK_RECV, SOCK_RESOLVE, SOCK_RESPONSE, SOCK_SEND, SOCK_UDP_RECV,
    SOCK_UDP_SEND,
};
#[cfg(target_os = "none")]
use hk_server_api::{
    close_handle, exit, frame_map, notify, recv_data, recv_data_handle, recv_data_try, send,
    send_data, service_endpoint, tsc, tsc_frequency, write, write_u64, yield_now,
};

#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 768 * 1024;
#[cfg(target_os = "none")]
const TCP_PORT: u16 = 45678;
/// 宿主 UDP echo 端口。
#[cfg(target_os = "none")]
const UDP_ECHO_PORT: u16 = 45679;
/// UDP 自检使用的本地端口。
#[cfg(target_os = "none")]
const UDP_LOCAL_PORT: u16 = 40001;
/// 对外报告的接口 MTU。
#[cfg(target_os = "none")]
const MTU: u16 = 1500;
/// Network Server 侧共享接收帧虚拟基址。
#[cfg(target_os = "none")]
const SERVER_RX_BASE: u64 = 0x3000_0000;
/// Network Server 侧共享发送帧虚拟基址。
#[cfg(target_os = "none")]
const SERVER_TX_BASE: u64 = 0x3010_0000;
#[cfg(target_os = "none")]
const WAIT_MS: u64 = 5_000;
#[cfg(target_os = "none")]
const DNS_WAIT_MS: u64 = 10_000;
#[cfg(target_os = "none")]
const POLL_WAIT_MS: u64 = 100;

/// 收发计数。
#[cfg(target_os = "none")]
#[derive(Default)]
struct Stats {
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
    let no_selftest = cmdline.is_some_and(|line| line.contains("net.no-selftest"));
    if !no_selftest {
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
            poll_once(driver, endpoint, &mut stack, &mut stats, &mut frame);
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
            poll_once(driver, endpoint, &mut stack, &mut stats, &mut frame);
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
            poll_once(driver, endpoint, &mut stack, &mut stats, &mut frame);
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
            poll_once(driver, endpoint, &mut stack, &mut stats, &mut frame);
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
            resolve(
                &mut stack,
                &[dns_server],
                name,
                driver,
                endpoint,
                &mut stats,
                &mut frame,
            );
        }
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

/// 把接口状态与收发计数编码为线协议字节串。
#[cfg(target_os = "none")]
fn status_blob(stack: &Stack, stats: &Stats, buffer: &mut [u8]) -> Option<usize> {
    let status = stack.status();
    NetStatus {
        mac: status.mac,
        ipv4: status.ipv4.map_or([0; 4], |address| address.octets()),
        has_ipv4: status.ipv4.is_some(),
        prefix: status.prefix,
        gateway: status.gateway.map_or([0; 4], |address| address.octets()),
        has_gateway: status.gateway.is_some(),
        sockets: status.sockets as u16,
        rx_frames: stats.rx_frames,
        tx_frames: stats.tx_frames,
        rx_bytes: stats.rx_bytes,
        tx_bytes: stats.tx_bytes,
    }
    .encode(buffer)
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

/// 输出 IPv4/IPv6 地址。
#[cfg(target_os = "none")]
fn write_address(address: &IpAddress) {
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

#[cfg(target_os = "none")]
fn poll_once(
    driver: u64,
    endpoint: u64,
    stack: &mut Stack,
    stats: &mut Stats,
    frame: &mut [u8; NET_FRAME_MAX],
) {
    // 先入队再通知：发送为异步，消息已在端点队列中，唤醒不会丢失。
    let _ = send(driver, NET_POLL, 0);
    let _ = notify(driver, NET_NOTIFY_TX);
    // 有界等待完成消息：即使偶发丢失 NET_POLL_DONE，也不会永久阻塞在接收上。
    let deadline = now_ms().saturating_add(POLL_WAIT_MS);
    loop {
        let (label, word, _sender, length) = recv_data_try(endpoint, frame);
        match label {
            NET_RX => {
                let length = length.min(frame.len());
                let _ = stack.push_frame(&frame[..length]);
                stats.rx_frames = stats.rx_frames.saturating_add(1);
                stats.rx_bytes = stats.rx_bytes.saturating_add(length as u64);
            }
            NET_RX_FRAME => {
                let length = net_rx_length(word);
                stats.rx_frames = stats.rx_frames.saturating_add(1);
                stats.rx_bytes = stats.rx_bytes.saturating_add(length as u64);
                stack.push_shared_rx(net_rx_index(word), net_rx_offset(word), length);
            }
            NET_TX_DONE => stack.shared_tx_done(net_tx_index(word)),
            NET_POLL_DONE => break,
            0 => {
                if now_ms() >= deadline {
                    break;
                }
                let _ = yield_now();
            }
            _ => {}
        }
    }
    stack.poll(now_ms());
    flush_tx(driver, stack, stats);
}

/// 是否启用共享帧路径。
#[cfg(target_os = "none")]
fn zerocopy_enabled() -> bool {
    let mut buffer = [0_u8; 128];
    hk_server_api::boot_cmdline(&mut buffer).is_some_and(|line| line.contains("net.zerocopy"))
}

/// 接收驱动移交的共享帧并映射到本任务地址空间。
#[cfg(target_os = "none")]
fn attach_shared(driver: u64, endpoint: u64, stack: &mut Stack) -> bool {
    let mut buffer = [0_u8; 8];
    let mut rx_seen = 0_usize;
    let mut tx_seen = 0_usize;
    while rx_seen < NET_SHARED_RX || tx_seen < NET_SHARED_TX {
        let (label, word, sender, _length, handle) =
            recv_data_handle(endpoint, &mut buffer, driver);
        if label != NET_FRAME_ATTACH || sender != driver || handle == 0 {
            return false;
        }
        let role = net_frame_role(word);
        let index = net_frame_index(word);
        let (base, count) = match role {
            NET_FRAME_ROLE_RX => (SERVER_RX_BASE, NET_SHARED_RX),
            NET_FRAME_ROLE_TX => (SERVER_TX_BASE, NET_SHARED_TX),
            _ => {
                let _ = close_handle(handle);
                return false;
            }
        };
        if index >= count {
            let _ = close_handle(handle);
            return false;
        }
        let virt = base + (index * NET_SHARED_FRAME) as u64;
        if frame_map(handle, virt) != NET_SHARED_FRAME as u64 {
            let _ = close_handle(handle);
            return false;
        }
        if role == NET_FRAME_ROLE_RX {
            rx_seen += 1;
        } else {
            tx_seen += 1;
        }
    }
    stack.enable_shared(
        SERVER_RX_BASE,
        SERVER_TX_BASE,
        NET_SHARED_FRAME,
        NET_SHARED_TX,
    );
    write("hk-server-net: shared frames ready\n".as_bytes());
    true
}

/// 处理一次原生 Socket 请求并回复。
#[cfg(target_os = "none")]
#[allow(clippy::too_many_arguments)]
fn handle_socket(
    label: u64,
    data: &[u8],
    sender: u64,
    driver: u64,
    endpoint: u64,
    stack: &mut Stack,
    stats: &mut Stats,
    dns_servers: &[Ipv4Addr],
    frame: &mut [u8; NET_FRAME_MAX],
    next_udp: &mut u16,
) {
    let mut reply = [0_u8; 8 + SOCK_PAYLOAD_MAX];
    let mut payload = 0_usize;
    let result = match label {
        SOCK_OPEN => {
            let id = match data.first().copied() {
                Some(SOCK_KIND_TCP) => stack.socket_open_tcp(2048),
                Some(SOCK_KIND_UDP) => {
                    let port = *next_udp;
                    *next_udp = next_udp.wrapping_add(1);
                    stack.socket_open_udp(port)
                }
                _ => None,
            };
            id.map_or(SOCK_ERROR, |id| id.0 as u64 + 1)
        }
        SOCK_CONNECT => match (socket_id(data, 0), read_ipv4(data, 8), read_u16(data, 12)) {
            (Some(id), Some(address), Some(port)) => {
                if stack.socket_connect(id, Ipv4Addr::from(address), port, 40000 + id.0 as u16) {
                    0
                } else {
                    SOCK_ERROR
                }
            }
            _ => SOCK_ERROR,
        },
        SOCK_SEND => match socket_id(data, 0) {
            Some(id) => stack.socket_send(id, &data[8..]).unwrap_or(0) as u64,
            None => SOCK_ERROR,
        },
        SOCK_RECV => {
            let max = read_u16(data, 8)
                .unwrap_or(SOCK_PAYLOAD_MAX as u16)
                .min(SOCK_PAYLOAD_MAX as u16) as usize;
            let mut buffer = [0_u8; SOCK_PAYLOAD_MAX];
            match socket_id(data, 0) {
                Some(id) => match stack.socket_recv(id, &mut buffer[..max]) {
                    Ok(Some(length)) => {
                        reply[8..8 + length].copy_from_slice(&buffer[..length]);
                        payload = length;
                        length as u64
                    }
                    Ok(None) => 0,
                    Err(_) => SOCK_ERROR,
                },
                None => SOCK_ERROR,
            }
        }
        SOCK_UDP_RECV => {
            let max = read_u16(data, 8)
                .unwrap_or(SOCK_PAYLOAD_MAX as u16)
                .min(SOCK_PAYLOAD_MAX as u16) as usize;
            let mut buffer = [0_u8; SOCK_PAYLOAD_MAX];
            match socket_id(data, 0) {
                Some(id) => match stack.socket_udp_recv(id, &mut buffer[..max]) {
                    Ok(Some(length)) => {
                        reply[8..8 + length].copy_from_slice(&buffer[..length]);
                        payload = length;
                        length as u64
                    }
                    Ok(None) => 0,
                    Err(_) => SOCK_ERROR,
                },
                None => SOCK_ERROR,
            }
        }
        SOCK_CLOSE => match socket_id(data, 0) {
            Some(id) => {
                stack.socket_close(id);
                0
            }
            None => SOCK_ERROR,
        },
        SOCK_UDP_SEND => match (socket_id(data, 0), read_ipv4(data, 8), read_u16(data, 12)) {
            (Some(id), Some(address), Some(port)) => {
                if stack.socket_udp_send(id, Ipv4Addr::from(address), port, &data[14..]) {
                    (data.len() - 14) as u64
                } else {
                    0
                }
            }
            _ => SOCK_ERROR,
        },
        SOCK_RESOLVE => {
            let name = core::str::from_utf8(data).unwrap_or("");
            let fallback = [Ipv4Addr::new(10, 0, 2, 3)];
            let servers = if dns_servers.is_empty() {
                &fallback[..]
            } else {
                dns_servers
            };
            match lookup(stack, servers, name, driver, endpoint, stats, frame) {
                Some(addresses) => {
                    let count = addresses.len().min(4);
                    for (index, address) in addresses.iter().take(count).enumerate() {
                        reply[8 + index * 4..8 + index * 4 + 4].copy_from_slice(&address.octets());
                    }
                    payload = count * 4;
                    count as u64
                }
                None => SOCK_ERROR,
            }
        }
        _ => SOCK_ERROR,
    };
    reply[..8].copy_from_slice(&result.to_le_bytes());
    let _ = send_data(sender, SOCK_RESPONSE, &reply[..8 + payload]);
    stack.poll(now_ms());
    flush_tx(driver, stack, stats);
    // 主动轮询驱动，确保握手/重传期间能及时取走接收帧。
    let _ = send(driver, NET_POLL, 0);
    let _ = notify(driver, NET_NOTIFY_TX);
}

#[cfg(target_os = "none")]
fn socket_id(data: &[u8], offset: usize) -> Option<SocketId> {
    let handle = read_u64(data, offset)?;
    (handle != 0).then(|| SocketId((handle - 1) as usize))
}

#[cfg(target_os = "none")]
fn read_u64(data: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        data.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

#[cfg(target_os = "none")]
fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        data.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

#[cfg(target_os = "none")]
fn read_ipv4(data: &[u8], offset: usize) -> Option<[u8; 4]> {
    data.get(offset..offset + 4)?.try_into().ok()
}

/// 用协议栈解析名称，返回最多 4 个 IPv4 地址。
#[cfg(target_os = "none")]
fn lookup(
    stack: &mut Stack,
    servers: &[Ipv4Addr],
    name: &str,
    driver: u64,
    endpoint: u64,
    stats: &mut Stats,
    frame: &mut [u8; NET_FRAME_MAX],
) -> Option<Vec<Ipv4Addr>> {
    let dns = stack.open_dns(servers).ok()?;
    let query = match stack.start_dns_query(dns, name, DnsQueryType::A) {
        Ok(query) => query,
        Err(_) => {
            stack.close_socket(dns);
            return None;
        }
    };
    let deadline = now_ms().saturating_add(DNS_WAIT_MS);
    let mut result = None;
    while now_ms() < deadline {
        poll_once(driver, endpoint, stack, stats, frame);
        match stack.poll_dns(dns, query) {
            Ok(Some(addresses)) => {
                result = Some(
                    addresses
                        .iter()
                        .filter_map(|address| match address {
                            IpAddress::Ipv4(address) => Some(*address),
                            _ => None,
                        })
                        .take(4)
                        .collect(),
                );
                break;
            }
            Ok(None) => {}
            Err(_) => break,
        }
    }
    stack.close_socket(dns);
    result
}

/// 回传共享环的归还/发送事件，并补发普通发送队列；随后唤醒驱动。
#[cfg(target_os = "none")]
fn flush_tx(driver: u64, stack: &mut Stack, stats: &mut Stats) {
    let mut rx_done = Vec::new();
    let mut tx_ready = Vec::new();
    stack.take_shared_events(&mut rx_done, &mut tx_ready);
    let mut woke = false;
    for index in rx_done {
        let _ = send(driver, NET_RX_REFILL, index as u64);
        woke = true;
    }
    for (index, length) in tx_ready {
        stats.tx_frames = stats.tx_frames.saturating_add(1);
        stats.tx_bytes = stats.tx_bytes.saturating_add(length as u64);
        let _ = send(driver, NET_TX_FRAME, net_tx_word(index, length));
        woke = true;
    }
    while let Some(packet) = stack.take_frame() {
        stats.tx_frames = stats.tx_frames.saturating_add(1);
        stats.tx_bytes = stats.tx_bytes.saturating_add(packet.len() as u64);
        let _ = send_data(driver, NET_TX, &packet);
        woke = true;
    }
    if woke {
        let _ = notify(driver, NET_NOTIFY_TX);
    }
}

/// 请求驱动退出，等待 Root Server 重启后重新握手并验证链路。
#[cfg(target_os = "none")]
fn restart_network(
    old: u64,
    endpoint: u64,
    stack: &mut Stack,
    stats: &mut Stats,
    frame: &mut [u8; NET_FRAME_MAX],
    gateway: Ipv4Addr,
) -> u64 {
    write("hk-server-net: driver restart\n".as_bytes());
    let _ = send(old, NET_SHUTDOWN, 0);
    let deadline = now_ms().saturating_add(WAIT_MS);
    let mut driver = old;
    while now_ms() < deadline {
        let _ = recv_data_try(endpoint, frame);
        let candidate = service_endpoint("hk-server-virtio-net");
        if candidate != 0 && candidate != old {
            driver = candidate;
            break;
        }
        let _ = yield_now();
    }
    if driver == old {
        write("hk-server-net: driver restart failed\n".as_bytes());
        return old;
    }

    let mut info = [0_u8; NET_INFO_BYTES];
    let _ = send(driver, NET_INFO_REQUEST, 0);
    let _ = notify(driver, NET_NOTIFY_TX);
    let deadline = now_ms().saturating_add(WAIT_MS);
    let mut rehandshaked = false;
    while now_ms() < deadline {
        let (label, _word, sender, length) = recv_data_try(endpoint, &mut info);
        if label == NET_INFO_RESPONSE && sender == driver && length >= NET_INFO_BYTES {
            rehandshaked = true;
            break;
        }
        let _ = yield_now();
    }
    if !rehandshaked {
        write("hk-server-net: driver rehandshake failed\n".as_bytes());
        return driver;
    }

    let Ok(icmp) = stack.open_icmp(0x1235) else {
        write("hk-server-net: restart icmp failed\n".as_bytes());
        return driver;
    };
    let _ = stack.send_ping(icmp, gateway, 0x1235, 1, b"holy");
    let deadline = now_ms().saturating_add(WAIT_MS);
    let mut reply = [0_u8; 64];
    while now_ms() < deadline {
        poll_once(driver, endpoint, stack, stats, frame);
        if let Ok(Some((length, _))) = stack.recv_ping(icmp, &mut reply)
            && length >= 8
        {
            stack.close_socket(icmp);
            write("hk-server-net: restart ok\n".as_bytes());
            return driver;
        }
    }
    stack.close_socket(icmp);
    write("hk-server-net: restart ping failed\n".as_bytes());
    driver
}

/// 解析原生启动期网络配置；`net.dhcp=off` 或 `net.ip=` 存在时返回静态配置。
#[cfg(target_os = "none")]
fn parse_static(line: &str) -> Option<(Ipv4Addr, u8, Option<Ipv4Addr>)> {
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
fn parse_resolvers(line: &str) -> Vec<Ipv4Addr> {
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

/// 以 TSC 换算单调毫秒时间；TSC 未知时退回累计 tick（约 10 ms）。
#[cfg(target_os = "none")]
fn now_ms() -> u64 {
    let frequency = tsc_frequency();
    if frequency == 0 {
        return hk_server_api::ticks().saturating_mul(10);
    }
    tsc().saturating_mul(1_000) / frequency
}

#[cfg(target_os = "none")]
fn wait_driver() -> u64 {
    hk_server_api::runtime::wait_service("hk-server-virtio-net").unwrap_or(0)
}

hk_server_api::server_boilerplate!();
