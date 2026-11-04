//! Network Server：smoltcp 协议栈与 virtio-net Endpoint 之间的桥接。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use core::net::Ipv4Addr;
#[cfg(target_os = "none")]
use core::panic::PanicInfo;

#[cfg(target_os = "none")]
use hk_net::Stack;
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::net::{
    NET_FRAME_MAX, NET_INFO_BYTES, NET_INFO_REQUEST, NET_INFO_RESPONSE, NET_NOTIFY_TX, NET_POLL,
    NET_POLL_DONE, NET_RX, NET_TX,
};
#[cfg(target_os = "none")]
use hk_server_api::{
    exit, notify, recv_data_from, send, send_data, service_endpoint, ticks, write, yield_now,
};

#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 768 * 1024;
#[cfg(target_os = "none")]
const TCP_PORT: u16 = 45678;

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
    let (mut stack, dhcp) = match Stack::new_dhcp(mac, ticks()) {
        Ok(stack) => stack,
        Err(_) => {
            write("hk-server-net: 协议栈初始化失败\n".as_bytes());
            exit(4)
        }
    };
    let mut frame = [0_u8; NET_FRAME_MAX];
    let deadline = ticks().saturating_add(500);
    let mut gateway = Ipv4Addr::new(10, 0, 2, 2);
    let mut leased = false;
    while ticks() < deadline {
        poll_once(driver, endpoint, &mut stack, &mut frame);
        if let Some(lease) = stack.poll_dhcp(dhcp) {
            gateway = lease.gateway.unwrap_or(gateway);
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
    let deadline = ticks().saturating_add(500);
    let mut reply = [0_u8; 64];
    let mut ping_ok = false;
    while ticks() < deadline {
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
    let connect_deadline = ticks().saturating_add(500);
    let mut connected = false;
    while ticks() < connect_deadline {
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
    let reply_deadline = ticks().saturating_add(500);
    while ticks() < reply_deadline {
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

    loop {
        let (label, _word, _sender, length) = recv_data_from(endpoint, driver, &mut frame);
        if label == NET_RX {
            let _ = stack.push_frame(&frame[..length.min(frame.len())]);
            stack.poll(ticks());
            while let Some(packet) = stack.take_frame() {
                let _ = send_data(driver, NET_TX, &packet);
            }
            let _ = notify(driver, NET_NOTIFY_TX);
        }
    }
}

#[cfg(target_os = "none")]
fn poll_once(driver: u64, endpoint: u64, stack: &mut Stack, frame: &mut [u8; NET_FRAME_MAX]) {
    let _ = send(driver, NET_POLL, 0);
    let _ = notify(driver, NET_NOTIFY_TX);
    loop {
        let (label, _word, _sender, length) = recv_data_from(endpoint, driver, frame);
        match label {
            NET_RX => {
                let _ = stack.push_frame(&frame[..length.min(frame.len())]);
            }
            NET_POLL_DONE => break,
            _ => {}
        }
    }
    stack.poll(ticks());
    while let Some(packet) = stack.take_frame() {
        let _ = send_data(driver, NET_TX, &packet);
    }
    let _ = notify(driver, NET_NOTIFY_TX);
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
