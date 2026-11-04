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
    NET_FRAME_MAX, NET_INFO_BYTES, NET_INFO_REQUEST, NET_INFO_RESPONSE, NET_POLL, NET_POLL_DONE,
    NET_RX, NET_TX,
};
#[cfg(target_os = "none")]
use hk_server_api::{
    exit, recv_data_from, send, send_data, service_endpoint, ticks, write, yield_now,
};

#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 768 * 1024;

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
    let (label, _word, _sender, length) = recv_data_from(endpoint, driver, &mut info);
    if label != NET_INFO_RESPONSE || length < NET_INFO_BYTES {
        write("hk-server-net: 接口信息失败\n".as_bytes());
        exit(3)
    }
    let mut mac = [0_u8; 6];
    mac.copy_from_slice(&info[..6]);
    let mut stack = match Stack::new(
        mac,
        Ipv4Addr::new(10, 0, 2, 15),
        24,
        Some(Ipv4Addr::new(10, 0, 2, 2)),
        ticks(),
    ) {
        Ok(stack) => stack,
        Err(_) => {
            write("hk-server-net: 协议栈初始化失败\n".as_bytes());
            exit(4)
        }
    };
    write("hk-server-net: ready\n".as_bytes());

    let mut frame = [0_u8; NET_FRAME_MAX];
    poll_once(driver, endpoint, &mut stack, &mut frame);
    loop {
        let (label, _word, _sender, length) = recv_data_from(endpoint, driver, &mut frame);
        if label == NET_RX {
            let _ = stack.push_frame(&frame[..length.min(frame.len())]);
            stack.poll(ticks());
            while let Some(packet) = stack.take_frame() {
                let _ = send_data(driver, NET_TX, &packet);
            }
        }
    }
}

#[cfg(target_os = "none")]
fn poll_once(driver: u64, endpoint: u64, stack: &mut Stack, frame: &mut [u8; NET_FRAME_MAX]) {
    let _ = send(driver, NET_POLL, 0);
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
