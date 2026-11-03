//! PCI 配置空间访问（机制 #1）。
//!
//! 通过 `0xCF8` 写入配置地址、`0xCFC` 读取数据，只支持 32 位对齐读取。

use crate::port::{inl, outl};

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;
const ENABLE: u32 = 1 << 31;

/// 读取配置空间中的一个 32 位字。
pub fn config_read(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    let address = ENABLE
        | (u32::from(bus) << 16)
        | (u32::from(device & 0x1F) << 11)
        | (u32::from(function & 0x07) << 8)
        | u32::from(offset & 0xFC);

    // SAFETY: PCI 配置端口在 x86_64 平台上固定存在；写入地址后读取数据端口。
    unsafe {
        outl(CONFIG_ADDRESS, address);
        inl(CONFIG_DATA)
    }
}
