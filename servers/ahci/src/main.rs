//! AHCI 块设备服务：按块设备类申请授权并暴露 Block Endpoint。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use hk_device::DeviceGrant;
#[cfg(target_os = "none")]
use hk_driver_ahci::Ahci;
#[cfg(target_os = "none")]
use hk_server_api::device::{DEVICE_CLASS_AHCI, DEVICE_GRANT, DEVICE_NONE, DEVICE_REQUEST};
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::{
    close_handle, device_info, dma_map_frame, exit, irq_bind, recv_handle, self_notification, send,
    service_endpoint, wait_notify, write, yield_now,
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
    write(b"hk-server-ahci: start\n");
    let manager = service_endpoint("hk-server-device");
    if manager == 0 {
        write(b"hk-server-ahci: no device manager\n");
        exit(0)
    }
    let Some(grant) = request_grant(endpoint, manager) else {
        write(b"hk-server-ahci: no AHCI device\n");
        exit(0)
    };
    let mut bytes = [0_u8; hk_device::DEVICE_GRANT_BYTES];
    let length = device_info(grant, &mut bytes) as usize;
    let Some(descriptor) = (length != u64::MAX as usize)
        .then(|| DeviceGrant::decode(&bytes[..length]))
        .flatten()
    else {
        write(b"hk-server-ahci: invalid grant\n");
        exit(2)
    };
    let Some(mut disk) = Ahci::probe(grant, &descriptor) else {
        write(b"hk-server-ahci: controller init failed\n");
        exit(3)
    };
    write(b"hk-server-ahci: sectors=");
    hk_server_api::format::Line::new()
        .number(disk.capacity_blocks())
        .text(b"\n")
        .finish();

    let notification = self_notification();
    let bound = irq_bind(grant, notification, 1) == 0;
    if bound {
        disk.enable_interrupts();
    }
    write(if bound {
        b"hk-server-ahci: irq bound\n"
    } else {
        b"hk-server-ahci: irq polling\n"
    });

    hk_block_ipc::serve_with_wait_and(
        endpoint,
        &mut disk,
        |virt, phys, length, frame| {
            if dma_map_frame(grant, frame, length) != u64::MAX {
                hk_driver_ahci::register_mapping(virt, phys, length);
            }
        },
        || {
            if bound {
                let _ = wait_notify(notification, 1);
            } else {
                yield_now();
            }
        },
        |_, _, _, _, _| {},
    );
    exit(0)
}

#[cfg(target_os = "none")]
fn request_grant(endpoint: u64, manager: u64) -> Option<u64> {
    let _ = send(manager, DEVICE_REQUEST, DEVICE_CLASS_AHCI);
    loop {
        let (label, _word, _sender, handle) = recv_handle(endpoint);
        if label == DEVICE_GRANT && handle != 0 {
            return Some(handle);
        }
        if label == DEVICE_NONE {
            return None;
        }
        if handle != 0 {
            let _ = close_handle(handle);
        }
    }
}

hk_server_api::server_boilerplate!();
