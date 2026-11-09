//! MMIO 寄存器访问：包装外部 `volatile-register` crate。
//!
//! 对外只暴露按字节偏移访问的 [`Mmio`] 窗口；`volatile-register` 的寄存器
//! 类型只在本 crate 内部出现。内核设备页与用户态驱动共用同一套读写语义。

#![no_std]

use volatile_register::{RO, RW};

/// 以字节偏移访问的 MMIO 窗口。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mmio {
    base: usize,
}

impl Mmio {
    /// 以虚拟基址构造窗口。
    ///
    /// # Safety
    /// `base` 必须指向已映射、且宽度满足访问要求的设备寄存器窗口。
    pub const unsafe fn new(base: usize) -> Self {
        Self { base }
    }

    /// 返回窗口基址。
    pub const fn base(&self) -> usize {
        self.base
    }

    /// 读取 8 位寄存器。
    pub fn read_u8(&self, offset: usize) -> u8 {
        // SAFETY: `new` 已要求窗口有效；偏移由调用方按寄存器布局给出。
        unsafe { self.ro::<u8>(offset).read() }
    }

    /// 读取 16 位寄存器。
    pub fn read_u16(&self, offset: usize) -> u16 {
        // SAFETY: 同上。
        unsafe { self.ro::<u16>(offset).read() }
    }

    /// 读取 32 位寄存器。
    pub fn read_u32(&self, offset: usize) -> u32 {
        // SAFETY: 同上。
        unsafe { self.ro::<u32>(offset).read() }
    }

    /// 读取 64 位寄存器。
    pub fn read_u64(&self, offset: usize) -> u64 {
        // SAFETY: 同上。
        unsafe { self.ro::<u64>(offset).read() }
    }

    /// 写入 8 位寄存器。
    ///
    /// # Safety
    /// 写入设备寄存器可能触发硬件副作用。
    pub unsafe fn write_u8(&self, offset: usize, value: u8) {
        // SAFETY: 由调用方保证寄存器可写。
        unsafe { self.rw::<u8>(offset).write(value) }
    }

    /// 写入 16 位寄存器。
    ///
    /// # Safety
    /// 写入设备寄存器可能触发硬件副作用。
    pub unsafe fn write_u16(&self, offset: usize, value: u16) {
        // SAFETY: 由调用方保证寄存器可写。
        unsafe { self.rw::<u16>(offset).write(value) }
    }

    /// 写入 32 位寄存器。
    ///
    /// # Safety
    /// 写入设备寄存器可能触发硬件副作用。
    pub unsafe fn write_u32(&self, offset: usize, value: u32) {
        // SAFETY: 由调用方保证寄存器可写。
        unsafe { self.rw::<u32>(offset).write(value) }
    }

    /// 写入 64 位寄存器。
    ///
    /// # Safety
    /// 写入设备寄存器可能触发硬件副作用。
    pub unsafe fn write_u64(&self, offset: usize, value: u64) {
        // SAFETY: 由调用方保证寄存器可写。
        unsafe { self.rw::<u64>(offset).write(value) }
    }

    /// 对 32 位寄存器执行读-改-写。
    ///
    /// # Safety
    /// 写入设备寄存器可能触发硬件副作用。
    pub unsafe fn update_u32<F>(&self, offset: usize, f: F)
    where
        F: FnOnce(u32) -> u32,
    {
        // SAFETY: 由调用方保证寄存器可写。
        unsafe { self.rw::<u32>(offset).modify(f) }
    }

    unsafe fn ro<T: Copy>(&self, offset: usize) -> &RO<T> {
        // SAFETY: 窗口有效；`RO` 与 `T` 布局相同且只读。
        unsafe { &*((self.base + offset) as *const RO<T>) }
    }

    unsafe fn rw<T: Copy>(&self, offset: usize) -> &RW<T> {
        // SAFETY: 窗口有效；`RW` 与 `T` 布局相同。
        unsafe { &*((self.base + offset) as *const RW<T>) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_base() {
        // SAFETY: 只读取窗口基址，不解引用。
        let mmio = unsafe { Mmio::new(0x1000) };
        assert_eq!(mmio.base(), 0x1000);
    }
}
