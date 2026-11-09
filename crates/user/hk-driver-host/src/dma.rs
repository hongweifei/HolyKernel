//! `dma-api::DmaOp` 的 HolyKernel 实现。

use alloc::vec::Vec;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU64, Ordering};

use dma_api::{
    DeviceDma, DmaAddr, DmaAllocHandle, DmaCoherency, DmaConstraints, DmaDeviceInfo, DmaDirection,
    DmaDomainId, DmaError, DmaMapHandle, DmaOp,
};
use hk_server_api::{
    CapabilityKind, capability_create, capability_phys, close_handle, dma_map_capability,
    map_attach, region_unmap,
};
use spin::Mutex;

const PAGE_SIZE: usize = 4096;

static GRANT: AtomicU64 = AtomicU64::new(0);

struct Allocation {
    dma: u64,
    capability: u64,
    region: u64,
}

static ALLOCATIONS: Mutex<Vec<Allocation>> = Mutex::new(Vec::new());

struct Mapping {
    virt: u64,
    phys: u64,
    len: usize,
}

static MAPPINGS: Mutex<Vec<Mapping>> = Mutex::new(Vec::new());

/// 登记 Block/设备服务映射的 Frame，供 streaming DMA 使用。
pub fn register_mapping(virt: u64, phys: u64, len: usize) {
    if len == 0 {
        return;
    }
    let mut mappings = MAPPINGS.lock();
    if let Some(entry) = mappings.iter_mut().find(|entry| entry.virt == virt) {
        entry.phys = phys;
        entry.len = len;
    } else {
        mappings.push(Mapping { virt, phys, len });
    }
}

fn translate(virt: u64, len: usize) -> Option<u64> {
    let end = virt.checked_add(len as u64)?;
    let mappings = MAPPINGS.lock();
    mappings.iter().find_map(|mapping| {
        let map_end = mapping.virt.checked_add(mapping.len as u64)?;
        (virt >= mapping.virt && end <= map_end).then_some(mapping.phys + (virt - mapping.virt))
    })
}

struct Region {
    virt: u64,
    phys: u64,
    capability: u64,
    region: u64,
}

fn create_region(layout: core::alloc::Layout) -> Option<Region> {
    let grant = GRANT.load(Ordering::Acquire);
    if grant == 0 || layout.size() == 0 || layout.align() > PAGE_SIZE {
        return None;
    }
    let capability = capability_create(CapabilityKind::DeviceAllocation, layout.size())?;
    let Some((region, virt)) = map_attach(capability) else {
        let _ = close_handle(capability);
        return None;
    };
    let Some((phys, length)) = capability_phys(capability) else {
        let _ = region_unmap(region);
        let _ = close_handle(capability);
        return None;
    };
    if length < layout.size() as u64
        || dma_map_capability(grant, capability, layout.size()) == u64::MAX
    {
        let _ = region_unmap(region);
        let _ = close_handle(capability);
        return None;
    }
    Some(Region {
        virt,
        phys,
        capability,
        region,
    })
}

/// `dma-api` 的 DMA 后端。
pub struct HostDma;

impl DmaOp for HostDma {
    fn page_size(&self) -> usize {
        PAGE_SIZE
    }

    unsafe fn alloc_contiguous(
        &self,
        constraints: DmaConstraints,
        layout: core::alloc::Layout,
    ) -> Option<DmaAllocHandle> {
        let _ = constraints;
        let region = create_region(layout)?;
        let cpu = NonNull::new(region.virt as *mut u8)?;
        ALLOCATIONS.lock().push(Allocation {
            dma: region.phys,
            capability: region.capability,
            region: region.region,
        });
        // SAFETY: create_region 返回的 CPU 视图与物理地址对应同一能力。
        Some(unsafe { DmaAllocHandle::new(cpu, cpu, DmaAddr::from(region.phys), layout) })
    }

    unsafe fn dealloc_contiguous(&self, handle: DmaAllocHandle) {
        let address = handle.dma_addr().as_u64();
        let allocation = {
            let mut allocations = ALLOCATIONS.lock();
            allocations
                .iter()
                .position(|entry| entry.dma == address)
                .map(|index| allocations.remove(index))
        };
        if let Some(allocation) = allocation {
            let _ = region_unmap(allocation.region);
            let _ = close_handle(allocation.capability);
        }
    }

    unsafe fn alloc_coherent(
        &self,
        constraints: DmaConstraints,
        layout: core::alloc::Layout,
    ) -> Option<DmaAllocHandle> {
        // SAFETY: 本平台的 DMA 能力是 cache coherent 的。
        unsafe { self.alloc_contiguous(constraints, layout) }
    }

    unsafe fn dealloc_coherent(&self, handle: DmaAllocHandle) -> Result<(), DmaError> {
        // SAFETY: 与 alloc_coherent 的分配配对。
        unsafe { self.dealloc_contiguous(handle) };
        Ok(())
    }

    unsafe fn map_streaming(
        &self,
        constraints: DmaConstraints,
        addr: NonNull<u8>,
        size: core::num::NonZeroUsize,
        direction: DmaDirection,
    ) -> Result<DmaMapHandle, DmaError> {
        let _ = (constraints, direction);
        let virt = addr.as_ptr() as u64;
        let len = size.get();
        let phys = translate(virt, len).ok_or(DmaError::NoMemory)?;
        let layout =
            core::alloc::Layout::from_size_align(len, 1).map_err(|_| DmaError::NoMemory)?;
        // SAFETY: translate 已确认该 CPU 区间属于已登记的 Frame。
        Ok(unsafe { DmaMapHandle::new(addr, DmaAddr::from(phys), layout, None) })
    }

    unsafe fn unmap_streaming(&self, _handle: DmaMapHandle) {}
}

static HOST: HostDma = HostDma;

/// 初始化 DMA 后端并返回设备视图。
pub fn init(grant: u64) -> DeviceDma {
    GRANT.store(grant, Ordering::Release);
    let info = DmaDeviceInfo::new(
        DmaDomainId::Direct,
        DmaCoherency::Coherent,
        DmaConstraints::new(u64::MAX),
    );
    DeviceDma::new(info, &HOST)
}
