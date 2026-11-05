//! virtio-net 驱动服务：接收 Device Manager 授权并暴露接口信息。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::string::ToString;

#[cfg(target_os = "none")]
use hk_driver_virtio::{Device, Network, probe, register_frame};
#[cfg(target_os = "none")]
use hk_hal::DeviceGrant;
#[cfg(target_os = "none")]
use hk_server_api::device::{DEVICE_CLASS_NETWORK, DEVICE_GRANT, DEVICE_NONE, DEVICE_REQUEST};
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::net::{
    NET_FRAME_ATTACH, NET_FRAME_MAX, NET_FRAME_ROLE_RX, NET_FRAME_ROLE_TX, NET_INFO_BYTES,
    NET_INFO_REQUEST, NET_INFO_RESPONSE, NET_NOTIFY_IRQ, NET_NOTIFY_TX, NET_POLL, NET_POLL_DONE,
    NET_RX, NET_RX_FRAME, NET_RX_REFILL, NET_SHARED_FRAME, NET_SHARED_RX, NET_SHARED_TX,
    NET_SHUTDOWN, NET_TX, NET_TX_DONE, NET_TX_FRAME, net_frame_word, net_rx_index, net_rx_word,
    net_tx_index, net_tx_length,
};
#[cfg(target_os = "none")]
use hk_server_api::{
    RIGHT_READ, RIGHT_TRANSFER, RIGHT_WRITE, device_info, dma_map_frame, exit, frame_alloc,
    frame_info, handle_duplicate, irq_bind, recv_data_try, recv_handle, self_notification, send,
    send_data, send_handle, service_endpoint, wait_notify, write, yield_now,
};

#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 256 * 1024;
/// 驱动侧共享接收帧虚拟基址。
#[cfg(target_os = "none")]
const SHARED_RX_BASE: u64 = 0x7000_0000;
/// 驱动侧共享发送帧虚拟基址。
#[cfg(target_os = "none")]
const SHARED_TX_BASE: u64 = 0x7010_0000;

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

/// 解析是否启用共享帧路径。
#[cfg(target_os = "none")]
fn zerocopy_enabled() -> bool {
    let mut buffer = [0_u8; 128];
    hk_server_api::boot_cmdline(&mut buffer).is_some_and(|line| line.contains("net.zerocopy"))
}

/// 分配共享帧、登记 DMA 并把副本句柄交给 Network Server；成功后提交接收缓冲。
#[cfg(target_os = "none")]
fn attach_shared(network: &mut Network, peer: u64, grant: u64) -> bool {
    if !attach_role(
        peer,
        grant,
        NET_FRAME_ROLE_RX,
        SHARED_RX_BASE,
        NET_SHARED_RX,
    ) || !attach_role(
        peer,
        grant,
        NET_FRAME_ROLE_TX,
        SHARED_TX_BASE,
        NET_SHARED_TX,
    ) {
        write("hk-server-virtio-net: shared frame attach failed\n".as_bytes());
        return false;
    }
    network.set_shared_layout(SHARED_RX_BASE, SHARED_TX_BASE, NET_SHARED_FRAME);
    for index in 0..NET_SHARED_RX {
        if !network.submit_shared_rx(index) {
            write("hk-server-virtio-net: shared rx submit failed\n".as_bytes());
            return false;
        }
    }
    write("hk-server-virtio-net: shared frames ready\n".as_bytes());
    true
}

#[cfg(target_os = "none")]
fn attach_role(peer: u64, grant: u64, role: u64, base: u64, count: usize) -> bool {
    for index in 0..count {
        let virt = base + (index * NET_SHARED_FRAME) as u64;
        let handle = frame_alloc(NET_SHARED_FRAME, virt);
        if handle == 0 || handle == u64::MAX {
            return false;
        }
        if let Some((phys, _)) = frame_info(handle) {
            register_frame(virt, phys, NET_SHARED_FRAME);
        }
        let _ = dma_map_frame(grant, handle, NET_SHARED_FRAME);
        let duplicate = handle_duplicate(handle, RIGHT_READ | RIGHT_WRITE | RIGHT_TRANSFER);
        if duplicate == u64::MAX {
            return false;
        }
        if send_handle(
            peer,
            NET_FRAME_ATTACH,
            net_frame_word(role, index),
            duplicate,
        ) == u64::MAX
        {
            return false;
        }
    }
    true
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
fn drain_rx(network: &hk_driver_virtio::Network, peer: u64) {
    if peer == 0 {
        return;
    }
    while let Some(frame) = network.receive() {
        let _ = send_data(peer, NET_RX, &frame);
    }
}

#[cfg(target_os = "none")]
fn drain_shared_rx(network: &hk_driver_virtio::Network, peer: u64) {
    if peer == 0 {
        return;
    }
    while let Some((index, offset, length)) = network.poll_shared_rx() {
        let _ = send(peer, NET_RX_FRAME, net_rx_word(index, offset, length));
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

hk_server_api::server_boilerplate!();
