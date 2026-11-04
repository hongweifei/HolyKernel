//! virtio-net 驱动服务：接收 Device Manager 授权并暴露接口信息。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::string::ToString;
#[cfg(target_os = "none")]
use core::panic::PanicInfo;

#[cfg(target_os = "none")]
use hk_driver_virtio::{Device, probe};
#[cfg(target_os = "none")]
use hk_hal::DeviceGrant;
#[cfg(target_os = "none")]
use hk_server_api::device::{DEVICE_CLASS_NETWORK, DEVICE_GRANT, DEVICE_NONE, DEVICE_REQUEST};
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::net::{NET_INFO_BYTES, NET_INFO_REQUEST, NET_INFO_RESPONSE, NET_SHUTDOWN};
#[cfg(target_os = "none")]
use hk_server_api::{
    device_info, exit, recv, recv_handle, send, send_data, service_endpoint, write,
};

#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 256 * 1024;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let manager = service_endpoint("hk-server-device");
    if manager == 0 {
        write("hk-server-virtio-net: 没有设备管理器\n".as_bytes());
        exit(2)
    }
    let _ = send(manager, DEVICE_REQUEST, DEVICE_CLASS_NETWORK);
    let Some(grant_handle) = wait_for_grant(endpoint) else {
        write("hk-server-virtio-net: 没有网络设备\n".as_bytes());
        exit(0)
    };
    let mut bytes = [0_u8; hk_hal::DEVICE_GRANT_BYTES];
    let length = device_info(grant_handle, &mut bytes) as usize;
    if length == u64::MAX as usize {
        write("hk-server-virtio-net: 授权描述读取失败\n".as_bytes());
        exit(2)
    }
    let Some(descriptor) = DeviceGrant::decode(&bytes[..length]) else {
        write("hk-server-virtio-net: 授权描述无效\n".as_bytes());
        exit(2)
    };
    match probe(grant_handle, descriptor) {
        Ok(Device::Network(network)) => {
            let mac = network.mac_address();
            write("hk-server-virtio-net: mac=".as_bytes());
            for (index, byte) in mac.iter().enumerate() {
                if index != 0 {
                    write(b":");
                }
                write_hex(*byte);
            }
            write(b"\n");
            serve(endpoint, network.mac_address())
        }
        Ok(_) => {
            write("hk-server-virtio-net: 授权设备不是网卡\n".as_bytes());
            exit(3)
        }
        Err(error) => {
            write("hk-server-virtio-net: ".as_bytes());
            write(error.to_string().as_bytes());
            write(b"\n");
            exit(4)
        }
    }
}

#[cfg(target_os = "none")]
fn wait_for_grant(endpoint: u64) -> Option<u64> {
    loop {
        let (label, _word, _sender, handle) = recv_handle(endpoint);
        if label == DEVICE_GRANT && handle != 0 {
            return Some(handle);
        }
        if label == DEVICE_NONE {
            return None;
        }
    }
}

#[cfg(target_os = "none")]
fn serve(endpoint: u64, mac: [u8; 6]) -> ! {
    loop {
        let (label, _word, sender) = recv(endpoint);
        match label {
            NET_INFO_REQUEST => {
                let mut info = [0_u8; NET_INFO_BYTES];
                info[..6].copy_from_slice(&mac);
                info[6..8].copy_from_slice(&1514_u16.to_le_bytes());
                info[8] = 1;
                let _ = send_data(sender, NET_INFO_RESPONSE, &info);
            }
            NET_SHUTDOWN => exit(0),
            _ => {}
        }
    }
}

#[cfg(target_os = "none")]
fn write_hex(value: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut buffer = [0_u8; 2];
    buffer[0] = HEX[usize::from(value >> 4)];
    buffer[1] = HEX[usize::from(value & 0xF)];
    write(&buffer);
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
    eprintln!("hk-server-virtio-net 只在 x86_64-unknown-none 目标上构建");
}
