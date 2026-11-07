//! Device Manager stub：通过通用 PCI 枚举打印设备清单。
//!
//! 本 crate 只在 `x86_64-unknown-none` 目标上构建；宿主构建保留占位入口，便于
//! `cargo clippy --workspace --all-targets` 与工作区测试。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use hk_device::{
    grant::{DEVICE_GRANT_BYTES, DeviceGrant, from_device},
    pci::{PciAddress, PciConfig},
    scan::{Bar, Device, bar_size, capabilities, enumerate, read_bars},
};
#[cfg(target_os = "none")]
hk_image::declare_service_role!(SERVICE_ROLE, hk_image::ServiceRole::device());

#[cfg(target_os = "none")]
use hk_server_api::device::{
    DEVICE_CLASS_ANY, DEVICE_CLASS_BLOCK, DEVICE_CLASS_CONSOLE, DEVICE_CLASS_INPUT,
    DEVICE_CLASS_NETWORK, DEVICE_CLASS_RNG, DEVICE_GRANT, DEVICE_NONE, DEVICE_REQUEST,
};
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

/// 缓存的 PCI 设备数量上限。
#[cfg(target_os = "none")]
const MAX_DEVICES: usize = 32;

/// 设备入口：枚举 PCI 设备并缓存授权，常驻等待驱动申请授权。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    // 启动时只枚举一次并缓存 DeviceGrant。`bar_size` 会临时关闭设备的命令
    // 寄存器解码并改写 BAR，若在驱动运行期间重复测量会打断正在工作的设备
    // （例如 virtio-blk 的容量读取与块 I/O）。
    let mut devices: [Option<(Device, DeviceGrant)>; MAX_DEVICES] = [None; MAX_DEVICES];
    let mut count = 0;
    enumerate(&SyscallConfig, 0, |device| {
        write_device(&device, 0, &mut false, DEVICE_CLASS_ANY);
        if count < MAX_DEVICES {
            devices[count] = Some((device, from_device(&SyscallConfig, &device)));
            count += 1;
        }
    });
    write("hk-server-device: 发现 ".as_bytes());
    write_u64(count as u64);
    write(" 个设备\n".as_bytes());

    loop {
        let (label, requested, sender) = recv(endpoint);
        if label != DEVICE_REQUEST {
            continue;
        }
        let class = requested & 0xFFFF_FFFF;
        let wanted = (requested >> 32) as usize;
        let mut matched = 0;
        let mut sent_grant = false;
        for entry in devices.iter().flatten() {
            let (device, grant) = entry;
            if !grant.is_virtio() || !class_matches(*device, class) {
                continue;
            }
            if matched == wanted {
                sent_grant = send_grant(grant, sender);
                break;
            }
            matched += 1;
        }
        if !sent_grant {
            let _ = send(sender, DEVICE_NONE, 0);
        }
    }
}

/// 判断缓存的设备是否匹配请求类别。
#[cfg(target_os = "none")]
fn class_matches(device: Device, requested: u64) -> bool {
    match requested {
        DEVICE_CLASS_ANY => true,
        DEVICE_CLASS_BLOCK => device.class == 0x01,
        DEVICE_CLASS_NETWORK => device.class == 0x02,
        // 过渡设备号并不总等于 `0x1000 + virtio 设备号`，这里显式列出。
        DEVICE_CLASS_INPUT => virtio_device(device, 0x1012, 0x1052),
        DEVICE_CLASS_RNG => virtio_device(device, 0x1005, 0x1044),
        DEVICE_CLASS_CONSOLE => virtio_device(device, 0x1003, 0x1043),
        _ => false,
    }
}

/// 判断设备是否为指定 virtio 设备（过渡号与现代号）。
#[cfg(target_os = "none")]
fn virtio_device(device: Device, legacy: u16, modern: u16) -> bool {
    device.vendor == 0x1AF4 && (device.device == legacy || device.device == modern)
}

/// 把缓存授权编码并交给申请者。
#[cfg(target_os = "none")]
fn send_grant(grant: &DeviceGrant, sender: u64) -> bool {
    let mut encoded = [0_u8; DEVICE_GRANT_BYTES];
    let Some(length) = grant.encode(&mut encoded) else {
        return false;
    };
    let handle = device_grant_create(&encoded[..length]);
    send_handle(sender, DEVICE_GRANT, 0, handle) == 0
}

#[cfg(target_os = "none")]
fn write_device(device: &Device, driver_endpoint: u64, sent_grant: &mut bool, requested: u64) {
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
            if let Some(size) = bar_size(&SyscallConfig, device, index) {
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
    let class_matches = match requested {
        DEVICE_CLASS_ANY => true,
        DEVICE_CLASS_BLOCK => device.class == 0x01,
        DEVICE_CLASS_NETWORK => device.class == 0x02,
        _ => false,
    };
    if !class_matches {
        write(b"\n");
        return;
    }
    let mut encoded = [0_u8; DEVICE_GRANT_BYTES];
    if let Some(length) = grant.encode(&mut encoded) {
        let handle = device_grant_create(&encoded[..length]);
        write(b"  grant-handle=");
        write_hex64(handle);
        if !*sent_grant {
            let transferred = send_handle(driver_endpoint, DEVICE_GRANT, 0, handle);
            write(b"  grant-transfer=");
            write_hex64(transferred);
            if transferred == 0 {
                *sent_grant = true;
            }
        }
    }
    write(b"\n");
}

#[cfg(target_os = "none")]
fn write_hex64(value: u64) {
    hk_server_api::format::write_hex(value, 16);
}

#[cfg(target_os = "none")]
fn write_hex(value: u32, digits: usize) {
    hk_server_api::format::write_hex(u64::from(value), digits);
}

hk_server_api::server_boilerplate!();
