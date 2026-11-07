//! HolyKernel 网络协议栈核心。
//!
//! 复用 `smoltcp` 实现 Ethernet、ARP、IPv4/IPv6、ICMP、UDP、TCP、DHCP 与 DNS；
//! 本 crate 提供 Frame 队列 PHY、静态接口与原生 Socket 包装。驱动侧通过 Net
//! Endpoint 把以太网帧送入 [`Stack::push_frame`]，并从 [`Stack::take_cow_page`] 取走发送帧。

#![no_std]

#[cfg(test)]
extern crate std;

extern crate alloc;

mod error;
mod phy;
mod stack;

#[cfg(test)]
mod tests;

pub use error::NetError;
pub use phy::FrameQueue;
pub use smoltcp::wire::{DnsQueryType, IpAddress};
pub use stack::{ArpEntry, SocketId, Stack, StackStatus};
