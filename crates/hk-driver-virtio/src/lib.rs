//! virtio 用户态驱动。
//!
//! 本 crate 在 [`hk_hal::DeviceGrant`] 之上接入 `virtio-drivers`：用 PCI 配置空间
//! 枚举 virtio 设备，用授权中的 MMIO 窗口映射寄存器，用授权中的 DMA 窗口分配
//! 共享缓冲。设备类别由 `virtio-drivers` 判定，块设备实现 [`hk_block::BlockDevice`]。

#![no_std]

extern crate alloc;

mod blk;
mod hal;
mod net;

use core::fmt;

use hk_hal::DeviceGrant;
use virtio_drivers::transport::DeviceType;
use virtio_drivers::transport::Transport;
use virtio_drivers::transport::pci::PciTransport;
use virtio_drivers::transport::pci::bus::PciRoot;
use virtio_drivers::transport::pci::virtio_device_type;

pub use blk::Block;
pub use hal::register_frame;
pub use net::Network;
pub use virtio_drivers::transport::DeviceType as VirtioDeviceType;

/// 探测到的 virtio 设备。
pub enum Device {
    /// 块设备。
    Block(alloc::boxed::Box<Block>),
    /// 网络设备。
    Network(alloc::boxed::Box<Network>),
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
    let function = root
        .enumerate_bus(0)
        .find(|(_, info)| virtio_device_type(info).is_some())
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
            let network = virtio_drivers::device::net::VirtIONet::<_, _, 16>::new(
                transport,
                net::RECEIVE_BUFFER,
            )
            .map_err(|_| ProbeError::Device)?;
            Ok(Device::Network(alloc::boxed::Box::new(Network::new(
                network,
            ))))
        }
        other => Ok(Device::Unsupported(other)),
    }
}
