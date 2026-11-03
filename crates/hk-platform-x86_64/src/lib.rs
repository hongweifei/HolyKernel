//! HolyKernel x86_64 平台。

#![no_std]

pub mod serial;

pub use serial::{COM1, Serial};
