//! PCI MSI-X：把设备中断从 INTx 改为投递到向量。
//!
//! 现状：capability 解析、表编程、INTx 关闭与 virtio 队列向量路由都已实现，
//! 中断也能到达绑定的向量；但启用后在 virtio-net 上观察到目标向量被高频
//! 反复投递，抢占 CPU 并拖慢其他服务，因此驱动服务暂不启用。
//!
//! 已知的下一步：
//! - 用专用的 MSI 向量（不复用 INTx 线路）并核对 `SYS_MSI_BIND` 的绑定结果；
//! - 在 QEMU 侧确认 `virtio_notify` 与中断数量的对应关系，定位重复投递来源。

use hk_base::IRQ_VECTOR_BASE;
use hk_mmio::Mmio;

use crate::pci_config::Config;

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
    let config = Config::new(&descriptor);

    let capability = config.find(MSIX_CAPABILITY)?;
    let control = config.read16(capability + 2);
    let entries = (control & 0x7FF) + 1;
    let table = config.read(capability + 4);
    let bar = (table & 0x07) as usize;
    let table_offset = u64::from(table & !0x07);
    let range = descriptor.mmio.get(bar).copied().flatten()?;
    if table_offset + 16 * u64::from(entries) > range.len {
        return None;
    }
    let table = crate::hal::mmio_virt(grant, &descriptor, bar)? + table_offset;

    // SAFETY: 表项落在已授权的 MSI-X BAR 内。
    let table = unsafe { Mmio::new(table as usize) };
    for entry in 0..u64::from(entries) {
        write_control(&table, entry * 16, 1);
    }
    // SAFETY: 表项落在已授权的 MSI-X BAR 内。
    unsafe {
        table.write_u32(8, u32::from(vector));
        table.write_u32(4, 0);
        table.write_u32(0, MSI_ADDRESS);
    }
    write_control(&table, 0, 0);

    config.write16(capability + 2, (control | 0x8000) & !0x4000);
    config.write(0x04, config.read(0x04) | 0x0400);
    route_queues(grant, &config, &descriptor);
    Some(Msix { entries, vector })
}

/// 在 virtio common config 中把全部队列指向 MSI-X 表项 0。
///
/// 队列未绑定表项时设备不会发中断；这里填的是表项索引，不是 CPU 向量。
fn route_queues(grant: u64, config: &Config, descriptor: &hk_device::DeviceGrant) {
    let Some(offset) = config.find(0x09) else {
        return;
    };
    if ((config.read(offset) >> 24) & 0xFF) != 1 {
        return;
    }
    let bar = u64::from(config.read(offset + 4) & 0xFF);
    if descriptor
        .mmio
        .get(bar as usize)
        .copied()
        .flatten()
        .is_none()
    {
        return;
    }
    let config_offset = u64::from(config.read(offset + 8));
    let Some(base) = crate::hal::mmio_virt(grant, descriptor, bar as usize) else {
        return;
    };
    // SAFETY: common config 落在已授权的 MMIO BAR 内。
    let common = unsafe { Mmio::new((base + config_offset) as usize) };
    // SAFETY: 队列选择与 MSI-X 表项索引写入设备寄存器。
    unsafe {
        let count = common.read_u16(18);
        for queue in 0..count {
            common.write_u16(22, queue);
            common.write_u16(26, 0);
        }
    }
}

fn write_control(table: &Mmio, offset: u64, value: u32) {
    // SAFETY: 表项落在已授权的 MSI-X BAR 内。
    unsafe { table.write_u32(offset as usize, value) };
}
