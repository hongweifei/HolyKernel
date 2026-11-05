//! virtio-net 驱动服务：接收 Device Manager 授权并暴露接口信息。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::string::ToString;

#[cfg(target_os = "none")]
use hk_driver_virtio::{Device, probe};
#[cfg(target_os = "none")]
use hk_hal::DeviceGrant;
#[cfg(target_os = "none")]
use hk_server_api::device::{DEVICE_CLASS_NETWORK, DEVICE_GRANT, DEVICE_NONE, DEVICE_REQUEST};
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::net::{
    NET_FRAME_MAX, NET_INFO_BYTES, NET_INFO_REQUEST, NET_INFO_RESPONSE, NET_NOTIFY_IRQ,
    NET_NOTIFY_TX, NET_POLL, NET_POLL_DONE, NET_RX_REFILL, NET_SHUTDOWN, NET_TX, NET_TX_DONE,
    NET_TX_FRAME, net_rx_index, net_tx_index, net_tx_length,
};
#[cfg(target_os = "none")]
use hk_server_api::{
    device_info, exit, irq_bind, recv_data_try, recv_handle, self_notification, send, send_data,
    service_endpoint, wait_notify, write, yield_now,
};

#[cfg(target_os = "none")]
mod shared;
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 256 * 1024;
#[cfg(target_os = "none")]
use shared::{attach_shared, drain_rx, drain_shared_rx, zerocopy_enabled};

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
    let wait = wait_for_grant(endpoint);
    let Some(grant_handle) = wait.grant else {
        write("hk-server-virtio-net: 没有网络设备\n".as_bytes());
        exit(0)
    };
    let mut peer = wait.pending.unwrap_or(0);
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
        Ok(Device::Network(mut network)) => {
            let mac = network.mac_address();
            write("hk-server-virtio-net: mac=".as_bytes());
            for (index, byte) in mac.iter().enumerate() {
                if index != 0 {
                    write(b":");
                }
                write_hex(*byte);
            }
            write(b"\n");
            if peer != 0 {
                send_info(peer, network.mac_address());
            }
            serve(endpoint, grant_handle, network.as_mut(), &mut peer)
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
struct GrantWait {
    grant: Option<u64>,
    pending: Option<u64>,
}

#[cfg(target_os = "none")]
fn wait_for_grant(endpoint: u64) -> GrantWait {
    let mut pending = None;
    loop {
        let (label, _word, sender, handle) = recv_handle(endpoint);
        if label == DEVICE_GRANT && handle != 0 {
            return GrantWait {
                grant: Some(handle),
                pending,
            };
        }
        if label == DEVICE_NONE {
            return GrantWait {
                grant: None,
                pending,
            };
        }
        pending = Some(sender);
    }
}

#[cfg(target_os = "none")]
fn serve(endpoint: u64, grant: u64, network: &mut hk_driver_virtio::Network, peer: &mut u64) -> ! {
    network.enable_interrupts();
    let notification = self_notification();
    let bound = irq_bind(grant, notification, NET_NOTIFY_IRQ) == 0;
    let zerocopy = zerocopy_enabled();
    let mut shared = false;
    if !zerocopy {
        network.enable_legacy_rx();
    } else if *peer != 0 {
        // 对端在等待授权期间已经发来 INFO 请求，`wait_for_grant` 消费了它，
        // 这里直接完成共享帧附加。
        shared = attach_shared(network, *peer, grant);
    }
    let mut buffer = [0_u8; NET_FRAME_MAX];
    loop {
        loop {
            let (label, word, sender, length) = recv_data_try(endpoint, &mut buffer);
            if label == 0 {
                break;
            }
            if sender != 0 {
                *peer = sender;
            }
            match label {
                NET_INFO_REQUEST => {
                    send_info(sender, network.mac_address());
                    if zerocopy && !shared {
                        shared = attach_shared(network, sender, grant);
                    }
                }
                NET_POLL => {
                    if shared {
                        drain_shared_rx(network, *peer);
                    } else {
                        drain_rx(network, *peer);
                    }
                    if *peer != 0 {
                        let _ = send_data(*peer, NET_POLL_DONE, &[]);
                    }
                }
                NET_TX if !shared => {
                    let length = length.min(buffer.len());
                    let _ = network.send(&buffer[..length]);
                }
                NET_TX_FRAME => {
                    let _ = network.submit_shared_tx(net_tx_index(word), net_tx_length(word));
                }
                NET_RX_REFILL => {
                    let _ = network.submit_shared_rx(net_rx_index(word));
                }
                NET_SHUTDOWN => exit(0),
                _ => {}
            }
        }
        if shared {
            drain_shared_rx(network, *peer);
            while let Some(index) = network.poll_shared_tx() {
                if *peer != 0 {
                    let _ = send(*peer, NET_TX_DONE, index as u64);
                }
            }
        } else {
            drain_rx(network, *peer);
            // 回收已完成的发送并补发待发帧；设备延迟完成时也不会阻塞。
            network.flush();
        }
        if bound {
            let bits = wait_notify(notification, NET_NOTIFY_IRQ | NET_NOTIFY_TX);
            if bits & NET_NOTIFY_IRQ != 0 {
                network.ack_interrupt();
            }
            if !shared {
                network.flush();
            }
        } else {
            let _ = yield_now();
        }
    }
}

#[cfg(target_os = "none")]
fn send_info(sender: u64, mac: [u8; 6]) {
    let mut info = [0_u8; NET_INFO_BYTES];
    info[..6].copy_from_slice(&mac);
    info[6..8].copy_from_slice(&1514_u16.to_le_bytes());
    info[8] = 1;
    let _ = send_data(sender, NET_INFO_RESPONSE, &info);
}

#[cfg(target_os = "none")]
fn write_hex(value: u8) {
    hk_server_api::format::write_hex(u64::from(value), 2);
}

hk_server_api::server_boilerplate!();
