//! hk-net 主机侧测试。

use alloc::vec;
use alloc::vec::Vec;
use core::net::{Ipv4Addr, Ipv6Addr};

use smoltcp::phy::{Device, RxToken, TxToken};
use smoltcp::time::Instant;

use crate::{DnsQueryType, FrameQueue, IpAddress, NetError, Stack};

const MAC_A: [u8; 6] = [0x02, 0, 0, 0, 0, 1];
const MAC_B: [u8; 6] = [0x02, 0, 0, 0, 0, 2];

#[test]
fn shared_queue_round_trips_and_recycles() {
    let mut rx_area = vec![0_u8; 4096];
    let mut tx_area = vec![0_u8; 8192];
    let mut queue = FrameQueue::new();
    queue.enable_shared(
        rx_area.as_mut_ptr() as u64,
        tx_area.as_mut_ptr() as u64,
        4096,
        2,
    );

    rx_area[..6].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
    queue.push_shared_rx(0, 0, 6);
    let (rx, tx) = queue.receive(Instant::from_millis(0)).expect("共享接收");
    rx.consume(|bytes| assert_eq!(bytes, &[1, 2, 3, 4, 5, 6]));
    tx.consume(4, |buffer| buffer.copy_from_slice(b"holy"));

    let mut done_rx = Vec::new();
    let mut done_tx = Vec::new();
    queue.take_shared_events(&mut done_rx, &mut done_tx);
    assert_eq!(done_rx, [0]);
    assert_eq!(done_tx, [(0, 4)]);
    assert_eq!(&tx_area[32..36], b"holy");

    // 取到但未 consume 的发送 token 不得泄漏帧。
    let _ = queue.transmit(Instant::from_millis(0));
    let token = queue.transmit(Instant::from_millis(0)).expect("仍有空闲帧");
    token.consume(1, |buffer| buffer[0] = 7);
    let mut done_rx = Vec::new();
    let mut done_tx = Vec::new();
    queue.take_shared_events(&mut done_rx, &mut done_tx);
    assert_eq!(done_tx, [(1, 1)]);
    assert_eq!(tx_area[4096 + 32], 7);
}

#[test]
fn queue_tokens_round_trip() {
    let mut queue = FrameQueue::new();
    assert!(queue.push_rx(b"rx"));
    let (rx, tx) = queue.receive(Instant::from_millis(0)).expect("接收 token");
    let received = rx.consume(|frame| frame.to_vec());
    assert_eq!(received, b"rx");
    tx.consume(2, |frame| frame.copy_from_slice(b"tx"));
    assert_eq!(queue.pop_tx().as_deref(), Some(&b"tx"[..]));
}

#[test]
fn answers_arp_request() {
    let mut stack = Stack::new(
        MAC_A,
        Ipv4Addr::new(10, 0, 2, 15),
        24,
        Some(Ipv4Addr::new(10, 0, 2, 2)),
        0x1234,
    )
    .expect("协议栈");
    let request = arp_request();
    stack.push_frame(&request).expect("注入 ARP");
    stack.poll(0);
    let reply = stack.take_frame().expect("ARP 回复");
    assert_eq!(&reply[0..6], &MAC_B);
    assert_eq!(&reply[6..12], &MAC_A);
    assert_eq!(&reply[12..14], &[0x08, 0x06]);
    assert_eq!(&reply[20..22], &[0x00, 0x02]);
    assert_eq!(&reply[22..28], &MAC_A);
    assert_eq!(&reply[28..32], &[10, 0, 2, 15]);
    assert_eq!(&reply[32..38], &MAC_B);
    assert_eq!(&reply[38..42], &[10, 0, 2, 2]);

    let entries = stack.arp_entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].ip, Ipv4Addr::new(10, 0, 2, 2));
    assert_eq!(entries[0].mac.0, MAC_B);
}

#[test]
fn dns_a_query_between_two_stacks() {
    let addresses = run_dns("holy.test", DnsQueryType::A, |query| {
        dns_reply(query, 0x80, 1, &[10, 0, 2, 99])
    })
    .expect("查询结果")
    .expect("已完成");
    assert_eq!(
        addresses.first(),
        Some(&IpAddress::Ipv4(Ipv4Addr::new(10, 0, 2, 99)))
    );
}

#[test]
fn dns_aaaa_query_between_two_stacks() {
    let address = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0x99);
    let addresses = run_dns("holy.test", DnsQueryType::Aaaa, |query| {
        dns_reply(query, 0x80, 28, &address.octets())
    })
    .expect("查询结果")
    .expect("已完成");
    assert_eq!(addresses.first(), Some(&IpAddress::Ipv6(address)));
}

#[test]
fn dns_failure_is_reported() {
    let result = run_dns("missing.test", DnsQueryType::A, |query| {
        dns_reply(query, 0x83, 1, &[])
    });
    assert_eq!(result, Err(NetError::Recv));
}

#[test]
fn dns_query_times_out() {
    let mut a = Stack::new(MAC_A, Ipv4Addr::new(10, 0, 2, 15), 24, None, 21).expect("A");
    let mut b = Stack::new(MAC_B, Ipv4Addr::new(10, 0, 2, 16), 24, None, 22).expect("B");
    let dns = a
        .open_dns(&[Ipv4Addr::new(10, 0, 2, 16)])
        .expect("DNS socket");
    let query = a
        .start_dns_query(dns, "holy.test", DnsQueryType::A)
        .expect("查询");
    let mut result = Ok(None);
    for step in 0..80 {
        let now = step * 200;
        a.poll(now);
        while let Some(frame) = a.take_frame() {
            b.push_frame(&frame).expect("A→B");
        }
        b.poll(now);
        while let Some(frame) = b.take_frame() {
            a.push_frame(&frame).expect("B→A");
        }
        result = a.poll_dns(dns, query);
        if !matches!(result, Ok(None)) {
            break;
        }
    }
    assert_eq!(result, Err(NetError::Recv));
}

#[test]
fn dns_rejects_empty_server_list() {
    let mut stack = Stack::new(MAC_A, Ipv4Addr::new(10, 0, 2, 15), 24, None, 21).expect("A");
    assert_eq!(stack.open_dns(&[]), Err(NetError::Address));
}

/// 让 A 向 B 发起一次 DNS 查询，由 `respond` 构造应答，返回最终查询结果。
fn run_dns(
    name: &str,
    query_type: DnsQueryType,
    respond: impl Fn(&[u8]) -> Vec<u8>,
) -> Result<Option<Vec<IpAddress>>, NetError> {
    let mut a = Stack::new(MAC_A, Ipv4Addr::new(10, 0, 2, 15), 24, None, 21).expect("A");
    let mut b = Stack::new(MAC_B, Ipv4Addr::new(10, 0, 2, 16), 24, None, 22).expect("B");
    let server = b.open_udp(53).expect("DNS UDP");
    let dns = a
        .open_dns(&[Ipv4Addr::new(10, 0, 2, 16)])
        .expect("DNS socket");
    let query = a.start_dns_query(dns, name, query_type).expect("查询");
    let mut request = [0_u8; 512];
    let mut result = Ok(None);
    for tick in 0..64 {
        a.poll(tick);
        while let Some(frame) = a.take_frame() {
            b.push_frame(&frame).expect("A→B");
        }
        b.poll(tick);
        if let Ok(Some((length, endpoint))) = b.recv_udp(server, &mut request)
            && let IpAddress::Ipv4(remote) = endpoint.addr
        {
            let response = respond(&request[..length]);
            b.send_udp(server, remote, endpoint.port, &response)
                .expect("DNS 响应");
        }
        while let Some(frame) = b.take_frame() {
            a.push_frame(&frame).expect("B→A");
        }
        a.poll(tick);
        result = a.poll_dns(dns, query);
        if !matches!(result, Ok(None)) {
            break;
        }
    }
    result
}

/// 构造 DNS 应答；`rcode` 为标志寄存器低字节，`rdata` 为空时不带应答记录。
fn dns_reply(query: &[u8], rcode: u8, record_type: u16, rdata: &[u8]) -> Vec<u8> {
    let mut offset = 12;
    while query[offset] != 0 {
        offset += 1 + usize::from(query[offset]);
    }
    offset += 5;
    let question = &query[12..offset];
    let answer_len = if rdata.is_empty() {
        0
    } else {
        12 + rdata.len()
    };
    let mut response = vec![0_u8; 12 + question.len() + answer_len];
    response[0..2].copy_from_slice(&query[0..2]);
    response[2..4].copy_from_slice(&[0x81, rcode]);
    response[4..6].copy_from_slice(&[0x00, 0x01]);
    response[6..8].copy_from_slice(&(u16::from(!rdata.is_empty())).to_be_bytes());
    response[12..12 + question.len()].copy_from_slice(question);
    if !rdata.is_empty() {
        let answer = 12 + question.len();
        response[answer..answer + 2].copy_from_slice(&[0xC0, 0x0C]);
        response[answer + 2..answer + 4].copy_from_slice(&record_type.to_be_bytes());
        response[answer + 4..answer + 6].copy_from_slice(&[0x00, 0x01]);
        response[answer + 6..answer + 10].copy_from_slice(&60_u32.to_be_bytes());
        response[answer + 10..answer + 12].copy_from_slice(&(rdata.len() as u16).to_be_bytes());
        response[answer + 12..].copy_from_slice(rdata);
    }
    response
}

#[test]
fn tcp_handshake_and_echo() {
    let mut a = Stack::new(MAC_A, Ipv4Addr::new(10, 0, 2, 15), 24, None, 11).expect("A");
    let mut b = Stack::new(MAC_B, Ipv4Addr::new(10, 0, 2, 16), 24, None, 12).expect("B");
    let listener = b.open_tcp(2048).expect("监听 socket");
    b.listen_tcp(listener, 4321).expect("监听");
    let client = a.open_tcp(2048).expect("客户端 socket");
    a.connect_tcp(client, Ipv4Addr::new(10, 0, 2, 16), 4321, 1234)
        .expect("连接");
    let mut connected = false;
    for tick in 0..64 {
        pump(&mut a, &mut b, tick);
        if a.tcp_may_send(client) && b.tcp_may_recv(listener) {
            connected = true;
            break;
        }
    }
    assert!(connected, "TCP 未建立");
    assert_eq!(a.send_tcp(client, b"tcp").expect("发送"), 3);
    for tick in 64..96 {
        pump(&mut a, &mut b, tick);
    }
    let mut buffer = [0_u8; 8];
    let length = b
        .recv_tcp(listener, &mut buffer)
        .expect("接收")
        .unwrap_or(0);
    assert_eq!(&buffer[..length], b"tcp");
}

fn pump(a: &mut Stack, b: &mut Stack, tick: u64) {
    a.poll(tick);
    while let Some(frame) = a.take_frame() {
        b.push_frame(&frame).expect("A→B");
    }
    b.poll(tick);
    while let Some(frame) = b.take_frame() {
        a.push_frame(&frame).expect("B→A");
    }
}

#[test]
fn udp_between_two_stacks() {
    let mut a = Stack::new(MAC_A, Ipv4Addr::new(10, 0, 2, 15), 24, None, 1).expect("A");
    let mut b = Stack::new(MAC_B, Ipv4Addr::new(10, 0, 2, 16), 24, None, 2).expect("B");
    let receiver = b.open_udp(4321).expect("绑定 B");
    let sender = a.open_udp(1234).expect("绑定 A");
    a.push_frame(&arp_reply()).expect("ARP 回复");
    a.poll(1);
    let mut sent = false;
    for tick in 0..16 {
        if !sent
            && a.send_udp(sender, Ipv4Addr::new(10, 0, 2, 16), 4321, b"hello")
                .is_ok()
        {
            sent = true;
        }
        a.poll(tick + 2);
        while let Some(frame) = a.take_frame() {
            b.push_frame(&frame).expect("B 收帧");
        }
        b.poll(tick + 2);
        while let Some(frame) = b.take_frame() {
            a.push_frame(&frame).expect("A 收帧");
        }
        let mut buffer = [0_u8; 16];
        if let Some((length, _)) = b.recv_udp(receiver, &mut buffer).expect("B 接收") {
            assert_eq!(&buffer[..length], b"hello");
            return;
        }
    }
    panic!("UDP 未送达");
}

fn arp_reply() -> [u8; 42] {
    let mut frame = arp_request();
    frame[0..6].copy_from_slice(&MAC_A);
    frame[6..12].copy_from_slice(&MAC_B);
    frame[20..22].copy_from_slice(&[0x00, 0x02]);
    frame[22..28].copy_from_slice(&MAC_B);
    frame[28..32].copy_from_slice(&[10, 0, 2, 16]);
    frame[32..38].copy_from_slice(&MAC_A);
    frame[38..42].copy_from_slice(&[10, 0, 2, 15]);
    frame
}

fn arp_request() -> [u8; 42] {
    let mut frame = [0_u8; 42];
    frame[0..6].copy_from_slice(&[0xff; 6]);
    frame[6..12].copy_from_slice(&MAC_B);
    frame[12..14].copy_from_slice(&[0x08, 0x06]);
    frame[14..16].copy_from_slice(&[0x00, 0x01]);
    frame[16..18].copy_from_slice(&[0x08, 0x00]);
    frame[18] = 6;
    frame[19] = 4;
    frame[20..22].copy_from_slice(&[0x00, 0x01]);
    frame[22..28].copy_from_slice(&MAC_B);
    frame[28..32].copy_from_slice(&[10, 0, 2, 2]);
    frame[32..38].copy_from_slice(&[0; 6]);
    frame[38..42].copy_from_slice(&[10, 0, 2, 15]);
    frame
}
