//! HolyKernel x86_64 平台。

#![no_std]

extern crate alloc;

pub mod cpu;
pub mod framebuffer;
pub mod paging;
pub mod port;
pub mod serial;

pub use framebuffer::FramebufferConsole;
pub use paging::{FrameAllocator, PageFlags, PageTable, PagingError};
pub use serial::{COM1, Serial};
