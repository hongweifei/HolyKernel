//! HolyKernel x86_64 平台。

#![no_std]

extern crate alloc;

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
pub mod timing;
pub mod vspace;

pub use hk_framebuffer::FramebufferConsole;
pub use paging::{PageAllocator, PageFlags, PageTable, PagingError};
pub use serial::{COM1, Serial};
pub use vspace::USER_VSPACES;
