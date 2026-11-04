//! hk-net 主机侧测试。

use core::net::Ipv4Addr;

use smoltcp::phy::{Device, RxToken, TxToken};
use smoltcp::time::Instant;

use crate::{FrameQueue, Stack};

const MAC_A: [u8; 6] = [0x02, 0, 0, 0, 0, 1];
const MAC_B: [u8; 6] = [0x02, 0, 0, 0, 0, 2];

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
