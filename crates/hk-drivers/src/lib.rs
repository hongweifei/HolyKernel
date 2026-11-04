//! 通用设备协议核心：PCI、16550 UART 与 PS/2 键盘。
//!
//! 只依赖 `hk-hal` 契约；端口/MMIO、ECAM 与中断由平台适配。

#![no_std]

#[cfg(test)]
extern crate alloc;

pub mod grant;
pub mod keyboard;
pub mod pci;
pub mod uart;
