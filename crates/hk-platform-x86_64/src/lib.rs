//! HolyKernel x86_64 平台。

#![no_std]

extern crate alloc;

pub mod cpu;
pub mod serial;

pub use serial::{COM1, Serial};
