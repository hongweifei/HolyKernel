//! virtio 驱动服务：按设备类别收集授权，块设备走块协议，熵源与控制台走辅助协议。
//!
//! 同一服务可以持有多块 virtio 设备；各设备的驱动封装在每次操作前切回自己的
//! 设备授权，因此共享的 HAL 状态不会在设备之间串用。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::boxed::Box;

#[cfg(target_os = "none")]
mod aux;
#[cfg(target_os = "none")]
use aux::Aux;

#[cfg(target_os = "none")]
use hk_block::BlockDevice;
#[cfg(target_os = "none")]
use hk_device::DeviceGrant;
#[cfg(target_os = "none")]
use hk_driver_virtio::{Block, Device, probe};
#[cfg(target_os = "none")]
use hk_proto::block::{BLOCK_REQUEST, BLOCK_RESPONSE, STATUS_ERROR, encode_response};
#[cfg(target_os = "none")]
use hk_server_api::device::{
    DEVICE_CLASS_BLOCK, DEVICE_CLASS_CONSOLE, DEVICE_CLASS_RNG, DEVICE_GRANT, DEVICE_NONE,
    DEVICE_REQUEST,
};
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::{
    close_handle, device_info, dma_map_frame, exit, irq_bind, recv_data_handle, recv_handle,
    self_notification, send, send_data, service_endpoint, wait_notify, write, yield_now,
};

/// 服务堆大小。
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 512 * 1024;

/// 块完成中断使用的通知位。
#[cfg(target_os = "none")]
const BLOCK_NOTIFY_IRQ: u64 = 1;

/// 辅助设备消息缓冲长度。
#[cfg(target_os = "none")]
const AUX_BUFFER: usize = 256;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

/// 启动时按类别收集到的设备授权。
#[cfg(target_os = "none")]
struct Grants {
    block: Option<u64>,
    rng: Option<u64>,
    console: Option<u64>,
}

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    write("hk-server-virtio: start\n".as_bytes());
    let manager = service_endpoint("hk-server-device");
    if manager == 0 {
        write("hk-server-virtio: 没有设备管理器\n".as_bytes());
        exit(2)
    }
    let grants = collect_grants(endpoint, manager);

    let mut block = None;
    if let Some(handle) = grants.block {
        match probe_grant(handle) {
            Some(Device::Block(device)) => block = Some((handle, device)),
            Some(_) => {
                write("hk-server-virtio: 授权设备不是块设备\n".as_bytes());
                exit(3)
            }
            None => {
                write("hk-server-virtio: 块设备初始化失败\n".as_bytes());
                exit(4)
            }
        }
    }

    let mut devices = Aux::default();
    if let Some(handle) = grants.rng {
        if let Some(Device::Rng(device)) = probe_grant(handle) {
            if aux::rng::self_test(&device) {
                write(b"hk-server-virtio: entropy ok\n");
            } else {
                write(b"hk-server-virtio: entropy failed\n");
            }
            write(b"hk-server-virtio: rng ready\n");
            devices.rng = Some(*device);
        } else {
            write("hk-server-virtio: 熵源初始化失败\n".as_bytes());
        }
    }
    if let Some(handle) = grants.console {
        if let Some(Device::Console(device)) = probe_grant(handle) {
            write(b"hk-server-virtio: console ready\n");
            if aux::console::self_test(&device) {
                write(b"hk-server-virtio: console echo ok\n");
            } else {
                write(b"hk-server-virtio: console echo timeout\n");
            }
            devices.console = Some(*device);
        } else {
            write("hk-server-virtio: 控制台初始化失败\n".as_bytes());
        }
    }

    if let Some((handle, device)) = block {
        serve_block(endpoint, handle, device, devices)
    } else if devices.rng.is_some() || devices.console.is_some() {
        serve_aux(endpoint, devices)
    } else {
        write("hk-server-virtio: 没有可服务的设备\n".as_bytes());
        exit(0)
    }
}

/// 依次向设备管理器申请块、熵源与控制台授权。
#[cfg(target_os = "none")]
fn collect_grants(endpoint: u64, manager: u64) -> Grants {
    Grants {
        block: request_grant(endpoint, manager, DEVICE_CLASS_BLOCK),
        rng: request_grant(endpoint, manager, DEVICE_CLASS_RNG),
        console: request_grant(endpoint, manager, DEVICE_CLASS_CONSOLE),
    }
}

/// 申请一类设备授权；等待期间到达的客户端请求回复「尚未就绪」。
#[cfg(target_os = "none")]
fn request_grant(endpoint: u64, manager: u64, class: u64) -> Option<u64> {
    let _ = send(manager, DEVICE_REQUEST, class);
    loop {
        let (label, _word, sender, handle) = recv_handle(endpoint);
        if label == DEVICE_GRANT && handle != 0 {
            return Some(handle);
        }
        if label == DEVICE_NONE {
            return None;
        }
        if handle != 0 {
            let _ = close_handle(handle);
        }
        reply_not_ready(sender, label);
    }
}

/// 回复块客户端「驱动尚未就绪」，让它可以重试。
#[cfg(target_os = "none")]
fn reply_not_ready(sender: u64, label: u64) {
    if label != BLOCK_REQUEST {
        return;
    }
    let mut response = [0_u8; 8];
    if let Some(length) = encode_response(&mut response, 0, STATUS_ERROR, &[]) {
        let _ = send_data(sender, BLOCK_RESPONSE, &response[..length]);
    }
}

/// 读取授权描述并探测设备。
#[cfg(target_os = "none")]
fn probe_grant(handle: u64) -> Option<Device> {
    let mut bytes = [0_u8; hk_device::DEVICE_GRANT_BYTES];
    let length = device_info(handle, &mut bytes) as usize;
    if length == u64::MAX as usize {
        return None;
    }
    let descriptor = DeviceGrant::decode(&bytes[..length])?;
    probe(handle, descriptor).ok()
}

/// 块设备主循环；辅助设备消息由 `devices` 分派。
#[cfg(target_os = "none")]
fn serve_block(endpoint: u64, grant: u64, mut block: Box<Block>, devices: Aux) -> ! {
    hk_server_api::format::Line::new()
        .text(b"hk-server-virtio: virtio-blk sectors=")
        .number(block.capacity_blocks())
        .text(b"\n")
        .finish();

    let notification = self_notification();
    let bound = irq_bind(grant, notification, BLOCK_NOTIFY_IRQ) == 0;
    if bound {
        block.enable_interrupts();
    }
    write(if bound {
        b"hk-server-virtio: block irq bound\n"
    } else {
        b"hk-server-virtio: block irq polling\n"
    });

    hk_block::ipc::serve_with_wait_and(
        endpoint,
        block.as_mut(),
        |virt, phys, length, frame| {
            if dma_map_frame(grant, frame, length) != u64::MAX {
                hk_driver_virtio::register_mapping(virt, phys, length);
            }
        },
        || {
            if bound {
                let _ = wait_notify(notification, BLOCK_NOTIFY_IRQ);
            } else {
                yield_now();
            }
        },
        |label, word, sender, data, received| {
            if received != 0 {
                let _ = close_handle(received);
            }
            devices.handle(label, word, sender, data);
        },
    );
    // 显式关闭视为需要重载的失败，供重启自检与热恢复使用。
    exit(1)
}

/// 没有块设备时只服务辅助设备。
#[cfg(target_os = "none")]
fn serve_aux(endpoint: u64, devices: Aux) -> ! {
    let mut buffer = [0_u8; AUX_BUFFER];
    loop {
        let (label, word, sender, length, received) = recv_data_handle(endpoint, &mut buffer, 0);
        if received != 0 {
            let _ = close_handle(received);
        }
        let length = length.min(buffer.len());
        devices.handle(label, word, sender, &buffer[..length]);
    }
}

hk_server_api::server_boilerplate!();
