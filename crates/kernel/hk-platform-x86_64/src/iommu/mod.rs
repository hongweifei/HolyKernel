//! x86_64 IOMMU 适配。
//!
//! 启动期若 DMAR 描述可用的 Intel VT-d 单元，就映射寄存器页并启用硬件
//! 翻译；否则保留软件 IOMMU，维持统一的域映射接口。

mod amd;
mod amd_second_stage;
mod second_stage;
mod vtd;

use alloc::vec::Vec;

use hk_fw::IommuKind;
use hk_fw::acpi::tables::Tables;
use hk_hal::VirtAddr;
use hk_hal::{Iommu, IommuDomain, IommuError, IommuFlags, Iova, PhysRange};
use hk_mem::SoftwareIommu;
use spin::Mutex;

use hk_mem::FrameBank;

use crate::paging::{PageFlags, PageTable};
use amd::AmdIommu;
use vtd::VtdIommu;

/// VT-d 寄存器页的内核虚拟窗口。
const IOMMU_MMIO_BASE: u64 = 0xFFFF_D000_0000_0000;
/// 每个 VT-d 单元的虚拟地址步长。
const IOMMU_MMIO_STRIDE: u64 = 4096;
/// AMD-Vi 寄存器页窗口，避免与已尝试的 VT-d 映射重叠。
const AMD_MMIO_BASE: u64 = 0xFFFF_D100_0000_0000;

enum Backend {
    Software(SoftwareIommu),
    Vtd(VtdIommu),
    Amd(AmdIommu),
}

/// 平台 IOMMU 后端。
pub struct PlatformIommu {
    backend: Mutex<Backend>,
}

impl PlatformIommu {
    const fn new() -> Self {
        Self {
            backend: Mutex::new(Backend::Software(SoftwareIommu::new())),
        }
    }
}

impl Iommu for PlatformIommu {
    fn map(
        &self,
        domain: IommuDomain,
        iova: Iova,
        phys: PhysRange,
        flags: IommuFlags,
    ) -> Result<(), IommuError> {
        match &*self.backend.lock() {
            Backend::Software(iommu) => iommu.map(domain, iova, phys, flags),
            Backend::Vtd(iommu) => iommu.map(domain, iova, phys, flags),
            Backend::Amd(iommu) => iommu.map(domain, iova, phys, flags),
        }
    }

    fn unmap(&self, domain: IommuDomain, iova: Iova, len: usize) -> Result<(), IommuError> {
        match &*self.backend.lock() {
            Backend::Software(iommu) => iommu.unmap(domain, iova, len),
            Backend::Vtd(iommu) => iommu.unmap(domain, iova, len),
            Backend::Amd(iommu) => iommu.unmap(domain, iova, len),
        }
    }

    fn flush(&self, domain: IommuDomain) -> Result<(), IommuError> {
        match &*self.backend.lock() {
            Backend::Software(iommu) => iommu.flush(domain),
            Backend::Vtd(iommu) => iommu.flush(domain),
            Backend::Amd(iommu) => iommu.flush(domain),
        }
    }

    fn translate(&self, domain: IommuDomain, iova: Iova) -> Option<(PhysRange, IommuFlags)> {
        match &*self.backend.lock() {
            Backend::Software(iommu) => iommu.translate(domain, iova),
            Backend::Vtd(iommu) => iommu.translate(domain, iova),
            Backend::Amd(iommu) => iommu.translate(domain, iova),
        }
    }

    fn translates_device_addresses(&self) -> bool {
        match &*self.backend.lock() {
            Backend::Software(iommu) => iommu.translates_device_addresses(),
            Backend::Vtd(iommu) => iommu.translates_device_addresses(),
            Backend::Amd(iommu) => iommu.translates_device_addresses(),
        }
    }
}

/// 全局 IOMMU 后端。
pub static IOMMU: PlatformIommu = PlatformIommu::new();

/// 按固件描述尝试启用 VT-d 或 AMD-Vi；没有可用单元时保持软件后端。
pub fn init(tables: &Tables, paging: &PageTable, allocator: &FrameBank) {
    let _ = allocator;
    let units = |kind: IommuKind| -> Vec<(u16, u64)> {
        tables
            .iommu
            .units
            .iter()
            .filter(|unit| unit.kind == kind)
            .map(|unit| (unit.segment, unit.base))
            .collect()
    };
    let intel = units(IommuKind::IntelVtd);
    if !intel.is_empty() {
        let mapped = map_units(paging, IOMMU_MMIO_BASE, intel.into_iter());
        if let Some(vtd) = VtdIommu::new(&mapped, crate::paging::direct_map_base()) {
            *IOMMU.backend.lock() = Backend::Vtd(vtd);
            return;
        }
    }
    let amd = units(IommuKind::AmdVi);
    if !amd.is_empty() {
        let mapped = map_units(paging, AMD_MMIO_BASE, amd.into_iter());
        if let Some(amd) = AmdIommu::new(&mapped, crate::paging::direct_map_base()) {
            *IOMMU.backend.lock() = Backend::Amd(amd);
        }
    }
}

fn map_units(paging: &PageTable, base: u64, units: impl Iterator<Item = (u16, u64)>) -> Vec<u64> {
    let mut mapped = Vec::new();
    for (index, (_segment, address)) in units.filter(|(segment, _)| *segment == 0).enumerate() {
        let virt = base + index as u64 * IOMMU_MMIO_STRIDE;
        if paging
            .map_mmio(
                VirtAddr::new(virt),
                hk_hal::PhysAddr::new(address),
                PageFlags::MMIO,
            )
            .is_ok()
        {
            mapped.push(virt);
        }
    }
    mapped
}
