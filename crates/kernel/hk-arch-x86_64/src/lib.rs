//! HolyKernel x86_64 平台。

#![no_std]

extern crate alloc;

pub mod address_space;
pub mod board;
pub mod bootstrap;
pub mod cache;
pub mod cpu;
pub mod dma;
pub mod entropy;
pub mod hpet;
pub mod ioapic;
pub mod iommu;
pub mod irq;
pub mod keyboard;
pub mod paging;
pub mod pci;
pub mod platform;
pub mod port;
pub mod rtc;
pub mod serial;
pub mod system;
pub mod system_arena;
pub mod timing;

pub use address_space::{USER_ADDRESS_SPACES, UserSpace};
pub use hk_framebuffer::FramebufferConsole;
pub use paging::{PageFlags, PageTable, PagingError};
pub use serial::{COM1, Serial};
