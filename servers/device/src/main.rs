//! Device Manager stub：通过系统调用读取 PCI 配置空间并打印设备清单。
//!
//! 本 crate 只在 `x86_64-unknown-none` 目标上构建；宿主构建保留占位入口，便于
//! `cargo clippy --workspace --all-targets` 与工作区测试。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;
#[cfg(target_os = "none")]
use hk_server_api::{exit, pci_read, write};

/// 配置空间偏移：厂商与设备标识。
#[cfg(target_os = "none")]
const VENDOR_DEVICE: u8 = 0x00;

/// 配置空间偏移：类代码与子类。
#[cfg(target_os = "none")]
const CLASS: u8 = 0x08;

/// 配置空间偏移：头类型。
#[cfg(target_os = "none")]
const HEADER_TYPE: u8 = 0x0C;

/// 设备入口：枚举全部总线的 PCI 设备。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(_endpoint: u64) -> ! {
    let mut count = 0;

    for bus in 0..=255_u16 {
        let bus = bus as u8;
        for device in 0..32_u8 {
            let first = pci_read(bus, device, 0, VENDOR_DEVICE);
            if first == u32::MAX {
                continue;
            }

            let header = pci_read(bus, device, 0, HEADER_TYPE) >> 16;
            let functions = if header & 0x80 != 0 { 8 } else { 1 };
            for function in 0..functions {
                let id = if function == 0 {
                    first
                } else {
                    pci_read(bus, device, function, VENDOR_DEVICE)
                };
                if id == u32::MAX {
                    continue;
                }
                write_device(bus, device, function, id);
                count += 1;
            }
        }
    }

    write("hk-server-device: 发现 ".as_bytes());
    write_number(count);
    write(" 个设备\n".as_bytes());
    exit(0)
}

#[cfg(target_os = "none")]
fn write_device(bus: u8, device: u8, function: u8, id: u32) {
    write(b"  ");
    write_hex(u32::from(bus), 2);
    write(b":");
    write_hex(u32::from(device), 2);
    write(b".");
    write_hex(u32::from(function), 1);
    write(b"  ");
    write_hex(id & 0xFFFF, 4);
    write(b":");
    write_hex(id >> 16, 4);

    let class = pci_read(bus, device, function, CLASS);
    let codes = ((class >> 24) << 8) | ((class >> 16) & 0xFF);
    write(b"  class=");
    write_hex(codes, 4);
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
fn write_number(mut value: u64) {
    if value == 0 {
        write(b"0");
        return;
    }

    let mut buffer = [0_u8; 20];
    let mut index = buffer.len();
    while value > 0 {
        index -= 1;
        buffer[index] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    write(&buffer[index..]);
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
