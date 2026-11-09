//! `mmio-api::MmioOp` 的 HolyKernel 实现。

use alloc::vec::Vec;
use core::ptr::NonNull;

use mmio_api::{MapError, Mmio, MmioAddr, MmioOp, MmioRaw};
use spin::Mutex;

struct Mapping {
    phys: u64,
    virt: u64,
    size: usize,
}

static MAPPINGS: Mutex<Vec<Mapping>> = Mutex::new(Vec::new());
static HOST: HostMmio = HostMmio;

/// 登记一个已通过 DeviceGrant 映射的设备页窗口。
pub fn register_mmio(phys: u64, virt: u64, size: usize) {
    let mut mappings = MAPPINGS.lock();
    if let Some(entry) = mappings.iter_mut().find(|entry| entry.phys == phys) {
        entry.virt = virt;
        entry.size = size;
    } else {
        mappings.push(Mapping { phys, virt, size });
    }
}

/// 返回全局 `MmioOp`，供需要自行 ioremap 的驱动 crate 使用。
pub fn op() -> &'static dyn MmioOp {
    &HOST
}

/// 初始化 `mmio-api` 的全局后端。
pub fn init() {
    mmio_api::init(&HOST);
}

/// 映射已登记的设备页窗口。
pub fn ioremap(phys: u64, size: usize) -> Option<Mmio> {
    mmio_api::ioremap(MmioAddr::from(phys), size).ok()
}

struct HostMmio;

impl MmioOp for HostMmio {
    fn ioremap(&self, addr: MmioAddr, size: usize) -> Result<MmioRaw, MapError> {
        let phys = addr.as_usize() as u64;
        let mappings = MAPPINGS.lock();
        let mapping = mappings
            .iter()
            .find(|mapping| {
                phys >= mapping.phys
                    && phys.saturating_add(size as u64) <= mapping.phys + mapping.size as u64
            })
            .ok_or(MapError::Invalid)?;
        let offset = phys - mapping.phys;
        let virt = NonNull::new((mapping.virt + offset) as *mut u8).ok_or(MapError::Invalid)?;
        // SAFETY: 调用方在放开映射前已通过 DeviceGrant 建立该窗口。
        Ok(unsafe { MmioRaw::new(addr, virt, size) })
    }

    fn iounmap(&self, _mmio: &MmioRaw) {
        // 设备窗口由 DeviceGrant 生命周期管理，不在此处解除。
    }
}
