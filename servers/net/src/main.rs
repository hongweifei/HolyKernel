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
use core::panic::PanicInfo;

#[cfg(target_os = "none")]
use hk_net::{DnsQueryType, IpAddress, Stack};
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::net::{
    NET_FRAME_MAX, NET_INFO_BYTES, NET_INFO_REQUEST, NET_INFO_RESPONSE, NET_NOTIFY_TX, NET_POLL,
    NET_POLL_DONE, NET_RX, NET_TX,
};
#[cfg(target_os = "none")]
use hk_server_api::{
    exit, notify, recv_data_from, recv_data_try, send, send_data, service_endpoint, tsc,
    tsc_frequency, write, write_u64, yield_now,
};

#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 768 * 1024;
#[cfg(target_os = "none")]
const TCP_PORT: u16 = 45678;
#[cfg(target_os = "none")]
const WAIT_MS: u64 = 5_000;
#[cfg(target_os = "none")]
const POLL_WAIT_MS: u64 = 100;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let driver = wait_driver();
    if driver == 0 {
        write("hk-server-net: 没有 virtio-net 驱动\n".as_bytes());
        exit(2)
    }

    let mut info = [0_u8; NET_INFO_BYTES];
    let _ = send(driver, NET_INFO_REQUEST, 0);
    let _ = notify(driver, NET_NOTIFY_TX);
    let (label, _word, _sender, length) = recv_data_from(endpoint, driver, &mut info);
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
    let mut frame = [0_u8; NET_FRAME_MAX];
    let deadline = now_ms().saturating_add(WAIT_MS);
    let mut gateway = Ipv4Addr::new(10, 0, 2, 2);
    let mut dns_servers: Vec<Ipv4Addr> = Vec::new();
    let mut leased = false;
    while now_ms() < deadline {
        poll_once(driver, endpoint, &mut stack, &mut frame);
        if let Some(lease) = stack.poll_dhcp(dhcp) {
            gateway = lease.gateway.unwrap_or(gateway);
            dns_servers = lease.dns;
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
    write("hk-server-net: ready\n".as_bytes());
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
        poll_once(driver, endpoint, &mut stack, &mut frame);
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
        poll_once(driver, endpoint, &mut stack, &mut frame);
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
        poll_once(driver, endpoint, &mut stack, &mut frame);
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

    // DNS 自检按需开启：`net.dns=<name>`。默认启动路径只验证 DHCP/ICMP/TCP。
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
            &mut frame,
        );
    }

    loop {
        let (label, _word, _sender, length) = recv_data_from(endpoint, driver, &mut frame);
        if label == NET_RX {
            let _ = stack.push_frame(&frame[..length.min(frame.len())]);
            stack.poll(now_ms());
            while let Some(packet) = stack.take_frame() {
                let _ = send_data(driver, NET_TX, &packet);
            }
            let _ = notify(driver, NET_NOTIFY_TX);
        }
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
    let deadline = now_ms().saturating_add(WAIT_MS);
    while now_ms() < deadline {
        poll_once(driver, endpoint, stack, frame);
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
fn write_hex(mut value: u16) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut buffer = [0_u8; 4];
    for index in (0..4).rev() {
        buffer[index] = HEX[usize::from(value & 0xF)];
        value >>= 4;
    }
    write(&buffer);
}

#[cfg(target_os = "none")]
fn poll_once(driver: u64, endpoint: u64, stack: &mut Stack, frame: &mut [u8; NET_FRAME_MAX]) {
    // 先入队再通知：发送为异步，消息已在端点队列中，唤醒不会丢失。
    let _ = send(driver, NET_POLL, 0);
    let _ = notify(driver, NET_NOTIFY_TX);
    // 有界等待完成消息：即使偶发丢失 NET_POLL_DONE，也不会永久阻塞在接收上。
    let deadline = now_ms().saturating_add(POLL_WAIT_MS);
    loop {
        let (label, _word, _sender, length) = recv_data_try(endpoint, frame);
        match label {
            NET_RX => {
                let _ = stack.push_frame(&frame[..length.min(frame.len())]);
            }
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
    while let Some(packet) = stack.take_frame() {
        let _ = send_data(driver, NET_TX, &packet);
    }
    let _ = notify(driver, NET_NOTIFY_TX);
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
    for _ in 0..4096 {
        let endpoint = service_endpoint("hk-server-virtio-net");
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
    eprintln!("hk-server-net 只在 x86_64-unknown-none 目标上构建");
}
