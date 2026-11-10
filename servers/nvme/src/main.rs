//! NVMe 块设备服务：使用 `nvme-driver` 与 RDIF 宿主桥。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use alloc::boxed::Box;

#[cfg(target_os = "none")]
use hk_device::DeviceGrant;
#[cfg(target_os = "none")]
use hk_driver_host::{init_dma, init_log, init_mmio, register_mapping, register_mmio};
#[cfg(target_os = "none")]
use hk_driver_rdif::{RdifDisk, Waiter};
#[cfg(target_os = "none")]
use hk_server_api::device::{DEVICE_CLASS_NVME, DEVICE_GRANT, DEVICE_NONE, DEVICE_REQUEST};
#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;
#[cfg(target_os = "none")]
use hk_server_api::{
    close_handle, device_info, device_map, dma_map, exit, irq_bind, recv_handle, self_notification,
    send, service_endpoint, wait_notify, write,
};
#[cfg(target_os = "none")]
use nvme_driver::{Config, Nvme, NvmeBlockDriver, NvmeIntxSource};

#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 512 * 1024;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

#[cfg(target_os = "none")]
struct IntxSource;

#[cfg(target_os = "none")]
impl NvmeIntxSource for IntxSource {
    fn is_asserted(&self) -> bool {
        true
    }
}

/// 服务入口。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    write(b"hk-server-nvme: start\n");
    let manager = service_endpoint("hk-server-device");
    if manager == 0 {
        write(b"hk-server-nvme: no device manager\n");
        exit(0)
    }
    let Some(grant) = request_grant(endpoint, manager) else {
        write(b"hk-server-nvme: no NVMe device\n");
        exit(0)
    };
    let mut bytes = [0_u8; hk_device::DEVICE_GRANT_BYTES];
    let length = device_info(grant, &mut bytes) as usize;
    let Some(descriptor) = (length != u64::MAX as usize)
        .then(|| DeviceGrant::decode(&bytes[..length]))
        .flatten()
    else {
        write(b"hk-server-nvme: invalid grant\n");
        exit(2)
    };
    let Some((index, range)) = descriptor
        .mmio
        .iter()
        .enumerate()
        .find_map(|(index, range)| range.map(|range| (index, range)))
    else {
        write(b"hk-server-nvme: no MMIO BAR\n");
        exit(2)
    };
    let Some(virt) = device_map(grant, index as u64) else {
        write(b"hk-server-nvme: MMIO map failed\n");
        exit(2)
    };
    let Ok(range_len) = usize::try_from(range.len) else {
        write(b"hk-server-nvme: BAR too large\n");
        exit(2)
    };

    init_log();
    init_mmio();
    register_mmio(range.base, virt, range_len);
    let dma = init_dma(grant);
    let Some(nvme) = Nvme::new(
        range.base,
        range_len,
        dma.clone(),
        hk_driver_host::mmio_op(),
        Config::intx(4096),
    )
    .ok() else {
        write(b"hk-server-nvme: controller init failed\n");
        exit(3)
    };

    let notification = self_notification();
    if irq_bind(grant, notification, 1) != 0 {
        write(b"hk-server-nvme: irq bind failed\n");
        exit(3)
    }
    let mut wait_irq = || {
        let _ = wait_notify(notification, 1);
    };
    let mut waiter = Waiter {
        wait_irq: &mut wait_irq,
    };
    let controller = NvmeBlockDriver::from_nvme(nvme).with_intx_source(IntxSource);
    let Ok(mut disk) = RdifDisk::open_controller(Box::new(controller), dma, &mut waiter) else {
        write(b"hk-server-nvme: block init failed\n");
        exit(3)
    };

    write(b"hk-server-nvme: sectors=");
    hk_server_api::format::Line::new()
        .number(disk.capacity_blocks())
        .text(b"\n")
        .finish();

    hk_block_ipc::serve_with_wait_and(
        endpoint,
        &mut disk,
        |virt, phys, length, frame| {
            if dma_map(grant, frame, length) != u64::MAX {
                register_mapping(virt, phys, length);
            }
        },
        || {
            let _ = wait_notify(notification, 1);
        },
        |_, _, _, _, _| {},
    );
    exit(0)
}

#[cfg(target_os = "none")]
fn request_grant(endpoint: u64, manager: u64) -> Option<u64> {
    let _ = send(manager, DEVICE_REQUEST, DEVICE_CLASS_NVME);
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
