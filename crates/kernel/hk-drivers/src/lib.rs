//! 通用设备协议核心：16550 UART 与 PS/2 键盘。
//!
//! 只依赖 `hk-hal` 的 I/O 与 Console 契约；端口/MMIO 访问由平台适配。设备描述
//! 与 PCI 枚举在 `hk-device`。

#![no_std]

pub mod keyboard;
pub mod uart;
