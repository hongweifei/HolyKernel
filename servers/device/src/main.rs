//! Device Manager stub：通过通用 PCI 枚举打印设备清单。
//!
//! 本 crate 只在 `x86_64-unknown-none` 目标上构建；宿主构建保留占位入口，便于
//! `cargo clippy --workspace --all-targets` 与工作区测试。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;
#[cfg(target_os = "none")]
use hk_pci::{Device, PciAddress, PciConfig, enumerate};
#[cfg(target_os = "none")]
use hk_server_api::{exit, pci_read, write, write_u64};

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

    fn write32(&self, _address: PciAddress, _value: u32) {}
}

/// 设备入口：枚举全部总线的 PCI 设备。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(_endpoint: u64) -> ! {
    let mut count = 0;
    enumerate(&SyscallConfig, 0, |device| {
        write_device(&device);
        count += 1;
    });

    write("hk-server-device: 发现 ".as_bytes());
    write_u64(count);
    write(" 个设备\n".as_bytes());
    exit(0)
}

#[cfg(target_os = "none")]
fn write_device(device: &Device) {
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
    write(b"\n");
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
