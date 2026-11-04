//! I/O APIC：中断重定向表配置。
//!
//! I/O APIC 通过 MMIO 访问：先向 `IOREGSEL` 写寄存器编号，再读写 `IOWIN`。
//! 每个重定向表项占两个 32 位寄存器，低半部分包含向量与投递属性。

use core::ptr::{read_volatile, write_volatile};

const IOREGSEL: u64 = 0x00;
const IOWIN: u64 = 0x10;
const IOAPICVER: u32 = 0x01;
const IOREDTBL: u32 = 0x10;

/// I/O APIC 寄存器页句柄。
pub struct IoApic {
    base: u64,
}

impl IoApic {
    /// 以 MMIO 虚拟基址构造句柄。
    pub const fn new(base: u64) -> Self {
        Self { base }
    }

    fn read(&self, register: u32) -> u32 {
        // SAFETY: 基址由调用方映射为设备页，寄存器偏移在 I/O APIC 页内。
        unsafe {
            write_volatile((self.base + IOREGSEL) as *mut u32, register);
            read_volatile((self.base + IOWIN) as *const u32)
        }
    }

    fn write(&self, register: u32, value: u32) {
        // SAFETY: 同上，写入 I/O APIC 选择寄存器与数据寄存器。
        unsafe {
            write_volatile((self.base + IOREGSEL) as *mut u32, register);
            write_volatile((self.base + IOWIN) as *mut u32, value);
        }
    }

    /// 返回版本寄存器值。
    pub fn version(&self) -> u32 {
        self.read(IOAPICVER)
    }

    /// 屏蔽全部重定向表项，返回版本寄存器值。
    ///
    /// 固件可能留下未屏蔽的表项；初始化时统一屏蔽，之后按需单独路由。
    pub fn init(&self) -> u32 {
        let version = self.version();
        let entries = ((version >> 16) & 0xFF) + 1;
        for index in 0..entries {
            let register = IOREDTBL + index * 2;
            self.write(register, 1 << 16);
            self.write(register + 1, 0);
        }
        version
    }

    /// 返回最大重定向表项编号。
    pub fn max_redirection_entry(&self) -> u32 {
        (self.version() >> 16) & 0xFF
    }

    /// 把全局系统中断路由到向量，目标为指定本地 APIC。
    ///
    /// 使用固定投递、物理目标、高有效、边沿触发与未屏蔽设置。
    pub fn route(&self, gsi: u32, gsi_base: u32, vector: u8, destination: u8) {
        self.write_route(gsi, gsi_base, vector, destination, 0);
    }

    /// 按 PCI 中断语义路由：低有效、电平触发。
    pub fn route_level(&self, gsi: u32, gsi_base: u32, vector: u8, destination: u8) {
        const ACTIVE_LOW_LEVEL: u32 = (1 << 13) | (1 << 15);
        self.write_route(gsi, gsi_base, vector, destination, ACTIVE_LOW_LEVEL);
    }

    fn write_route(&self, gsi: u32, gsi_base: u32, vector: u8, destination: u8, flags: u32) {
        let Some(index) = gsi.checked_sub(gsi_base) else {
            return;
        };
        if index > self.max_redirection_entry() {
            return;
        }

        let register = IOREDTBL + index * 2;
        let high = u32::from(destination) << 24;
        // 低半部分：向量号与触发属性，屏蔽位清零。
        let low = u32::from(vector) | flags;
        self.write(register + 1, high);
        self.write(register, low);
    }
}
