//! PCI MSI-X：把设备中断从 INTx 改为投递到向量。
//!
//! 现状：capability 解析、表编程、INTx 关闭与 virtio 队列向量路由都已实现，
//! 中断也能到达绑定的向量；但启用后在 virtio-net 上观察到目标向量被高频
//! 反复投递，抢占 CPU 并拖慢其他服务，因此驱动服务暂不启用。
//!
//! 已知的下一步：
//! - 用专用的 MSI 向量（不复用 INTx 线路）并核对 `SYS_MSI_BIND` 的绑定结果；
//! - 在 QEMU 侧确认 `virtio_notify` 与 MSI 数量的对应关系，定位重复投递来源。

use core::ptr::write_volatile;

use hk_hal::irq::IRQ_VECTOR_BASE;
use hk_server_api::{pci_read, pci_write};

/// MSI 目标：xAPIC 物理目的地模式，目标 APIC 为 0。
const MSI_ADDRESS: u32 = 0xFEE0_0000;
/// MSI-X capability 的 ID。
const MSIX_CAPABILITY: u8 = 0x11;

/// 启用结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Msix {
    /// 表项数。
    pub entries: u16,
    /// 设备中断使用的向量。
    pub vector: u8,
}

/// 为当前驱动实例的设备启用 MSI-X，表项 0 承载 `IRQ_VECTOR_BASE + irq`。
///
/// 返回 `None` 表示设备没有 MSI-X 或表无法映射，调用方继续使用 INTx。
pub fn enable(grant: u64) -> Option<Msix> {
    let descriptor = crate::hal::grant_descriptor()?;
    let line = descriptor.irq?;
    let vector = IRQ_VECTOR_BASE + line;
    let address = descriptor.address;
    let read = |offset: u8| pci_read(address.bus, address.device, address.function, offset);
    let write_config = |offset: u8, value: u32| {
        pci_write(
            address.bus,
            address.device,
            address.function,
            offset & 0xFC,
            value,
        );
    };
    let read16 = |offset: u8| {
        let word = read(offset & 0xFC);
        (word >> ((offset & 3) * 8)) as u16
    };
    let write16 = |offset: u8, value: u16| {
        let shift = u32::from(offset & 3) * 8;
        let current = read(offset & 0xFC);
        let mask = 0xFFFF_u32 << shift;
        write_config(offset, (current & !mask) | (u32::from(value) << shift));
    };

    let capability = find_capability(&read, MSIX_CAPABILITY)?;
    let control = read16(capability + 2);
    let entries = (control & 0x7FF) + 1;
    let table = read(capability + 4);
    let bar = (table & 0x07) as usize;
    let table_offset = u64::from(table & !0x07);
    let range = descriptor.mmio.get(bar).copied().flatten()?;
    if table_offset + 16 * u64::from(entries) > range.len {
        return None;
    }
    let base = crate::hal::mmio_virt(grant, &descriptor, bar)?;
    let table = base + table_offset;

    for entry in 0..u64::from(entries) {
        write_control(table + entry * 16, 1);
    }
    // SAFETY: 表项落在已授权的 MSI-X BAR 内。
    unsafe {
        write_volatile((table + 8) as *mut u32, u32::from(vector));
        write_volatile((table + 4) as *mut u32, 0);
        write_volatile(table as *mut u32, MSI_ADDRESS);
    }
    write_control(table, 0);

    write16(capability + 2, (control | 0x8000) & !0x4000);
    write_config(0x04, read(0x04) | 0x0400);
    route_queues(grant, &read, &descriptor);
    Some(Msix { entries, vector })
}

/// 遍历配置空间 capability 链，返回指定 ID 的偏移。
fn find_capability(read: &impl Fn(u8) -> u32, id: u8) -> Option<u8> {
    let mut offset = (read(0x34) & 0xFC) as u8;
    for _ in 0..48 {
        if offset == 0 {
            break;
        }
        let header = read(offset);
        if (header & 0xFF) as u8 == id {
            return Some(offset);
        }
        offset = ((header >> 8) & 0xFC) as u8;
    }
    None
}

/// 在 virtio common config 中把全部队列指向 MSI-X 表项 0。
///
/// 队列未绑定表项时设备不会发中断；这里填的是表项索引，不是 CPU 向量。
fn route_queues(grant: u64, read: &impl Fn(u8) -> u32, descriptor: &hk_hal::DeviceGrant) {
    let Some(offset) = find_capability(read, 0x09) else {
        return;
    };
    if ((read(offset) >> 24) & 0xFF) != 1 {
        return;
    }
    let bar = u64::from(read(offset + 4) & 0xFF);
    if descriptor
        .mmio
        .get(bar as usize)
        .copied()
        .flatten()
        .is_none()
    {
        return;
    }
    let config_offset = u64::from(read(offset + 8));
    let Some(base) = crate::hal::mmio_virt(grant, descriptor, bar as usize) else {
        return;
    };
    let common = base + config_offset;
    // SAFETY: common config 落在已授权的 MMIO BAR 内。
    unsafe {
        let count = core::ptr::read_volatile((common + 18) as *const u16);
        for queue in 0..count {
            write_volatile((common + 22) as *mut u16, queue);
            write_volatile((common + 26) as *mut u16, 0);
        }
    }
}

fn write_control(table: u64, value: u32) {
    // SAFETY: 表项落在已授权的 MSI-X BAR 内。
    unsafe { write_volatile((table + 12) as *mut u32, value) };
}
