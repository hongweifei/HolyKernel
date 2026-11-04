//! Device Manager stub：通过通用 PCI 枚举打印设备清单。
//!
//! 本 crate 只在 `x86_64-unknown-none` 目标上构建；宿主构建保留占位入口，便于
//! `cargo clippy --workspace --all-targets` 与工作区测试。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;
#[cfg(target_os = "none")]
use hk_drivers::grant::{DEVICE_GRANT_BYTES, from_device};
#[cfg(target_os = "none")]
use hk_drivers::pci::{Bar, Device, PciAddress, PciConfig, capabilities, enumerate, read_bars};
#[cfg(target_os = "none")]
#[used]
#[unsafe(link_section = ".hk_service")]
static SERVICE_ROLE: hk_server_api::ServiceRole = hk_server_api::ServiceRole::device();

#[cfg(target_os = "none")]
#[cfg(target_os = "none")]
use hk_server_api::block::{BLOCK_GRANT, BLOCK_NO_DEVICE, DEVICE_REQUEST};
#[cfg(target_os = "none")]
use hk_server_api::{device_grant_create, pci_read, recv, send, send_handle, write, write_u64};

/// 通过系统调用访问 PCI 配置空间。
#[cfg(target_os = "none")]
struct SyscallConfig;

#[cfg(target_os = "none")]
impl PciConfig for SyscallConfig {
    fn read32(&self, address: PciAddress) -> u32 {
        pci_read(
            address.bus,
            address.device,
            address.function,
            address.offset,
        )
    }

    fn write32(&self, address: PciAddress, value: u32) {
        hk_server_api::pci_write(
            address.bus,
            address.device,
            address.function,
            address.offset,
            value,
        );
    }
}

/// 设备入口：枚举 PCI 设备，常驻等待驱动申请授权。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    let mut count = 0;
    enumerate(&SyscallConfig, 0, |device| {
        write_device(&device, 0, &mut false);
        count += 1;
    });
    write("hk-server-device: 发现 ".as_bytes());
    write_u64(count);
    write(" 个设备\n".as_bytes());

    loop {
        let (label, _word, sender) = recv(endpoint);
        if label != DEVICE_REQUEST {
            continue;
        }
        let mut sent_grant = false;
        enumerate(&SyscallConfig, 0, |device| {
            write_device(&device, sender, &mut sent_grant);
        });
        if !sent_grant {
            let _ = send(sender, BLOCK_NO_DEVICE, 0);
        }
    }
}

#[cfg(target_os = "none")]
fn write_device(device: &Device, driver_endpoint: u64, sent_grant: &mut bool) {
    write(b"  ");
    write_hex(u32::from(device.address.bus), 2);
    write(b":");
    write_hex(u32::from(device.address.device), 2);
    write(b".");
    write_hex(u32::from(device.address.function), 1);
    write(b"  ");
    write_hex(u32::from(device.vendor), 4);
    write(b":");
    write_hex(u32::from(device.device), 4);

    let class = (u32::from(device.class) << 8) | u32::from(device.subclass);
    write(b"  class=");
    write_hex(class, 4);

    for (index, bar) in read_bars(&SyscallConfig, device).iter().enumerate() {
        if let Some(bar) = bar {
            write(b"  bar");
            write_hex(index as u32, 1);
            write(b"=");
            match bar {
                Bar::Io { base } => {
                    write(b"io:");
                    write_hex(*base, 4);
                }
                Bar::Memory {
                    base,
                    prefetchable,
                    is_64bit,
                } => {
                    write(if *is_64bit { b"mem64:" } else { b"mem:" });
                    write_hex64(*base);
                    if *prefetchable {
                        write(b":prefetch");
                    }
                }
            }
            if let Some(size) = hk_drivers::pci::bar_size(&SyscallConfig, device, index) {
                write(b" size=");
                write_hex64(size);
            }
        }
    }
    capabilities(&SyscallConfig, device, |cap| {
        write(b"  cap=");
        write_hex(u32::from(cap.id), 2);
    });

    let grant = from_device(&SyscallConfig, device);
    for (index, range) in grant.mmio.iter().enumerate() {
        if let Some(range) = range {
            write(b"  grant-mmio");
            write_hex(index as u32, 1);
            write(b"=");
            write_hex64(range.base);
            write(b"+");
            write_hex64(range.len);
        }
    }
    for (index, range) in grant.io.iter().enumerate() {
        if let Some(range) = range {
            write(b"  grant-io");
            write_hex(index as u32, 1);
            write(b"=");
            write_hex(u32::from(range.base), 4);
            write(b"+");
            write_hex(u32::from(range.len), 4);
        }
    }
    if let Some(irq) = grant.irq {
        write(b"  grant-irq=");
        write_hex(u32::from(irq), 2);
    }
    if grant.is_virtio() {
        write(b"  virtio");
    }
    if !grant.is_virtio() || driver_endpoint == 0 {
        write(b"\n");
        return;
    }
    let mut encoded = [0_u8; DEVICE_GRANT_BYTES];
    if let Some(length) = grant.encode(&mut encoded) {
        let handle = device_grant_create(&encoded[..length]);
        write(b"  grant-handle=");
        write_hex64(handle);
        if !*sent_grant {
            let transferred = send_handle(driver_endpoint, BLOCK_GRANT, 0, handle);
            write(b"  grant-transfer=");
            write_hex64(transferred);
            *sent_grant = transferred == 0;
        }
    }
    write(b"\n");
}

#[cfg(target_os = "none")]
fn write_hex64(value: u64) {
    write_hex((value >> 32) as u32, 8);
    write_hex(value as u32, 8);
}

#[cfg(target_os = "none")]
fn write_hex(value: u32, digits: usize) {
    let mut buffer = [0_u8; 8];
    for (index, slot) in buffer.iter_mut().enumerate().take(digits) {
        let shift = (digits - 1 - index) * 4;
        let nibble = ((value >> shift) & 0xF) as u8;
        *slot = if nibble < 10 {
            b'0' + nibble
        } else {
            b'a' + nibble - 10
        };
    }
    write(&buffer[..digits]);
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
    eprintln!("hk-server-device 只在 x86_64-unknown-none 目标上构建");
}
