//! virtio 用户态驱动。
//!
//! 本 crate 在 [`hk_hal::DeviceGrant`] 之上接入 `virtio-drivers`：用 PCI 配置空间
//! 枚举 virtio 设备，用授权中的 MMIO 窗口映射寄存器，用授权中的 DMA 窗口分配
//! 共享缓冲。设备类别由 `virtio-drivers` 判定，已接入块、网络、输入、熵源与
//! 控制台设备；块设备实现 [`hk_block::BlockDevice`]。
//!
//! 同一进程可以持有多个设备授权：每个设备封装在操作前切回自己的授权，MMIO
//! 映射按 `(授权, BAR)` 缓存。熵源与控制台的线协议与客户端在 [`protocol`]。

#![no_std]

extern crate alloc;

mod blk;
mod console;
mod hal;
mod input;
pub mod msi;
pub mod msix;
mod net;
mod pci_config;
pub mod protocol;
mod rng;

use core::fmt;

use hk_hal::DeviceGrant;
use virtio_drivers::transport::DeviceType;
use virtio_drivers::transport::Transport;
use virtio_drivers::transport::pci::PciTransport;
use virtio_drivers::transport::pci::bus::PciRoot;
use virtio_drivers::transport::pci::virtio_device_type;

pub use blk::Block;
pub use console::Console;
pub use hal::{grant_descriptor, register_mapping};
pub use input::Input;
pub use msi::Msi;
pub use msix::Msix;
pub use net::Network;
pub use rng::Rng;
pub use virtio_drivers::transport::DeviceType as VirtioDeviceType;

/// virtio 驱动服务名；熵源与控制台协议共用同一服务端点。
pub const SERVICE_NAME: &str = "hk-server-virtio";

/// 探测到的 virtio 设备。
pub enum Device {
    /// 块设备。
    Block(alloc::boxed::Box<Block>),
    /// 网络设备。
    Network(alloc::boxed::Box<Network>),
    /// 输入设备。
    Input(alloc::boxed::Box<Input>),
    /// 熵源设备。
    Rng(alloc::boxed::Box<Rng>),
    /// 控制台设备。
    Console(alloc::boxed::Box<Console>),
    /// `virtio-drivers` 支持、当前驱动服务尚未接入的设备类别。
    Unsupported(VirtioDeviceType),
}

/// 探测错误。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeError {
    /// PCI 总线上没有 virtio 设备。
    NoDevice,
    /// PCI transport 初始化失败。
    Transport,
    /// 设备初始化失败。
    Device,
}

impl fmt::Display for ProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NoDevice => "未找到 virtio 设备",
            Self::Transport => "virtio PCI transport 初始化失败",
            Self::Device => "virtio 设备初始化失败",
        })
    }
}

impl core::error::Error for ProbeError {}

/// 使用设备授权探测一个 virtio 设备。
///
/// 驱动实例在进程内是单例：授权、DMA 窗口与 MMIO 映射由模块级状态保存。
pub fn probe(grant: u64, descriptor: DeviceGrant) -> Result<Device, ProbeError> {
    hal::bind(grant, descriptor);
    let mut root = PciRoot::new(hal::SysConfig);
    let address = descriptor.address;
    let function = root
        .enumerate_bus(address.bus)
        .find(|(function, info)| {
            function.device == address.device
                && function.function == address.function
                && virtio_device_type(info).is_some()
        })
        .map(|(function, _)| function)
        .ok_or(ProbeError::NoDevice)?;
    let transport = PciTransport::new::<hal::HalImpl, _>(&mut root, function)
        .map_err(|_| ProbeError::Transport)?;
    let kind = transport.device_type();
    match kind {
        DeviceType::Block => {
            let block = virtio_drivers::device::blk::VirtIOBlk::new(transport)
                .map_err(|_| ProbeError::Device)?;
            Ok(Device::Block(alloc::boxed::Box::new(Block::new(block))))
        }
        DeviceType::Network => {
            let raw = virtio_drivers::device::net::VirtIONetRaw::<_, _, { net::QUEUE_SIZE }>::new(
                transport,
            )
            .map_err(|_| ProbeError::Device)?;
            let network = Network::new(raw).map_err(|_| ProbeError::Device)?;
            Ok(Device::Network(alloc::boxed::Box::new(network)))
        }
        DeviceType::Input => {
            let input = Input::new(transport).map_err(|_| ProbeError::Device)?;
            Ok(Device::Input(alloc::boxed::Box::new(input)))
        }
        DeviceType::EntropySource => {
            let rng = virtio_drivers::device::rng::VirtIORng::new(transport)
                .map_err(|_| ProbeError::Device)?;
            Ok(Device::Rng(alloc::boxed::Box::new(Rng::new(rng))))
        }
        DeviceType::Console => {
            let console = virtio_drivers::device::console::VirtIOConsole::new(transport)
                .map_err(|_| ProbeError::Device)?;
            Ok(Device::Console(alloc::boxed::Box::new(Console::new(
                console,
            ))))
        }
        other => Ok(Device::Unsupported(other)),
    }
}
