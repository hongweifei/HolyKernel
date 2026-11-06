//! virtio 驱动服务：接收 Device Manager 移交的设备授权并暴露块设备协议。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::string::ToString;

#[cfg(target_os = "none")]
use hk_block::BlockDevice;
#[cfg(target_os = "none")]
use hk_block::ipc::serve_with_wait;
#[cfg(target_os = "none")]
use hk_driver_virtio::{Device, probe};
#[cfg(target_os = "none")]
use hk_hal::DeviceGrant;
#[cfg(target_os = "none")]
use hk_server_api::block::{
    BLOCK_GRANT, BLOCK_NO_DEVICE, BLOCK_RESPONSE, DEVICE_REQUEST, STATUS_ERROR, encode_response,
};
#[cfg(target_os = "none")]
use hk_server_api::device::DEVICE_CLASS_BLOCK;
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::{
    device_info, dma_map_frame, exit, irq_bind, recv_handle, self_notification, send, send_data,
    service_endpoint, wait_notify, write, write_u64, yield_now,
};

/// 服务堆大小。
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 512 * 1024;

/// 块完成中断使用的通知位。
#[cfg(target_os = "none")]
const BLOCK_NOTIFY_IRQ: u64 = 1;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

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
    let _ = send(manager, DEVICE_REQUEST, DEVICE_CLASS_BLOCK);
    let wait = wait_for_grant(endpoint);
    let Some(grant_handle) = wait.grant else {
        reply_not_ready(wait.pending);
        exit(0)
    };
    let mut bytes = [0_u8; hk_hal::DEVICE_GRANT_BYTES];
    let length = device_info(grant_handle, &mut bytes) as usize;
    if length == u64::MAX as usize {
        write("hk-server-virtio: 授权描述读取失败\n".as_bytes());
        reply_not_ready(wait.pending);
        exit(2)
    }
    let Some(descriptor) = DeviceGrant::decode(&bytes[..length]) else {
        write("hk-server-virtio: 授权描述无效\n".as_bytes());
        reply_not_ready(wait.pending);
        exit(2)
    };

    match probe(grant_handle, descriptor) {
        Ok(Device::Block(mut block)) => {
            write("hk-server-virtio: virtio-blk sectors=".as_bytes());
            write_u64(block.capacity_blocks());
            write(b"\n");
            reply_not_ready(wait.pending);
            let notification = self_notification();
            let bound = irq_bind(grant_handle, notification, BLOCK_NOTIFY_IRQ) == 0;
            if bound {
                block.enable_interrupts();
            }
            write(if bound {
                b"hk-server-virtio: block irq bound\n"
            } else {
                b"hk-server-virtio: block irq polling\n"
            });
            serve_with_wait(
                endpoint,
                block.as_mut(),
                |virt, phys, length, frame| {
                    if dma_map_frame(grant_handle, frame, length) != u64::MAX {
                        hk_driver_virtio::register_frame(virt, phys, length);
                    }
                },
                || {
                    if bound {
                        let _ = wait_notify(notification, BLOCK_NOTIFY_IRQ);
                    } else {
                        yield_now();
                    }
                },
            );
            // 显式关闭视为需要重载的失败，供重启自检与热恢复使用。
            exit(1)
        }
        Ok(Device::Network(_)) => {
            write("hk-server-virtio: 网络设备需要 hk-server-virtio-net\n".as_bytes());
            reply_not_ready(wait.pending);
            exit(6)
        }
        Ok(Device::Input(_)) => {
            write("hk-server-virtio: 输入设备需要 hk-server-input\n".as_bytes());
            reply_not_ready(wait.pending);
            exit(6)
        }
        Ok(Device::Unsupported(kind)) => {
            write("hk-server-virtio: 尚未支持的设备类型 ".as_bytes());
            write_u64(kind as u64);
            write(b"\n");
            reply_not_ready(wait.pending);
            exit(3)
        }
        Err(error) => {
            write("hk-server-virtio: ".as_bytes());
            write(error.to_string().as_bytes());
            write(b"\n");
            reply_not_ready(wait.pending);
            exit(4)
        }
    }
}

/// 等待设备授权期间暂存提前到达的块请求发送者。
#[cfg(target_os = "none")]
struct GrantWait {
    grant: Option<u64>,
    pending: Option<u64>,
}

/// 等待设备授权；提前到达的块请求只记下发送者，等驱动就绪后再回复。
#[cfg(target_os = "none")]
fn wait_for_grant(endpoint: u64) -> GrantWait {
    let mut pending = None;
    loop {
        let (label, _word, sender, handle) = recv_handle(endpoint);
        if label == BLOCK_GRANT && handle != 0 {
            return GrantWait {
                grant: Some(handle),
                pending,
            };
        }
        if label == BLOCK_NO_DEVICE {
            write("hk-server-virtio: 没有设备授权\n".as_bytes());
            return GrantWait {
                grant: None,
                pending,
            };
        }
        pending = Some(sender);
    }
}

/// 回复「驱动尚未就绪」，让客户端可以重试。
#[cfg(target_os = "none")]
fn reply_not_ready(pending: Option<u64>) {
    let Some(sender) = pending else {
        return;
    };
    let mut response = [0_u8; 8];
    if let Some(length) = encode_response(&mut response, 0, STATUS_ERROR, &[]) {
        let _ = send_data(sender, BLOCK_RESPONSE, &response[..length]);
    }
}

hk_server_api::server_boilerplate!();
