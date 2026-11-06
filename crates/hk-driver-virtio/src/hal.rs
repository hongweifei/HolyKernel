//! `virtio-drivers` 的 HAL 适配：MMIO、DMA 与 PCI 配置空间都来自系统调用。

use alloc::vec::Vec;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU64, Ordering};

use hk_hal::DeviceGrant;
use hk_server_api::{device_map, dma_alloc, dma_sync, pci_read, pci_write};
use spin::Mutex;
use virtio_drivers::transport::pci::bus::{ConfigurationAccess, DeviceFunction};
use virtio_drivers::{BufferDirection, Hal, PhysAddr};

/// 页大小。
const PAGE: usize = 4096;

static GRANT: AtomicU64 = AtomicU64::new(0);
static GRANT_DESC: Mutex<Option<DeviceGrant>> = Mutex::new(None);
static SHARED: Mutex<Vec<Shared>> = Mutex::new(Vec::new());
static FREE: Mutex<Vec<Free>> = Mutex::new(Vec::new());
static FRAMES: Mutex<Vec<FrameRegion>> = Mutex::new(Vec::new());
static MMIO: Mutex<Vec<Option<u64>>> = Mutex::new(Vec::new());

struct Shared {
    phys: u64,
    virt: u64,
    len: usize,
    borrowed: bool,
}

struct FrameRegion {
    virt: u64,
    phys: u64,
    len: usize,
}

/// 登记共享映射的用户虚拟地址与物理区间，供 DMA 直接引用。
pub fn register_mapping(virt: u64, phys: u64, len: usize) {
    let mut frames = FRAMES.lock();
    if let Some(region) = frames.iter_mut().find(|region| region.virt == virt) {
        region.phys = phys;
        region.len = len;
    } else {
        frames.push(FrameRegion { virt, phys, len });
    }
}

struct Free {
    pages: usize,
    phys: u64,
    virt: u64,
}

/// 绑定本次驱动实例使用的设备授权。
pub fn bind(grant: u64, descriptor: DeviceGrant) {
    GRANT.store(grant, Ordering::Release);
    *GRANT_DESC.lock() = Some(descriptor);
}

/// 当前驱动实例的设备授权描述。
pub fn grant_descriptor() -> Option<DeviceGrant> {
    *GRANT_DESC.lock()
}

/// `virtio-drivers` 在当前任务内的 HAL 实现。
pub struct HalImpl;

impl HalImpl {
    fn alloc_pages(pages: usize) -> Option<(u64, u64)> {
        {
            let mut free = FREE.lock();
            if let Some(index) = free.iter().position(|entry| entry.pages == pages) {
                let entry = free.swap_remove(index);
                return Some((entry.phys, entry.virt));
            }
        }
        let len = pages.checked_mul(PAGE)?;
        let grant = GRANT.load(Ordering::Acquire);
        if grant == 0 {
            return None;
        }
        dma_alloc(grant, len)
    }

    fn release_pages(pages: usize, phys: u64, virt: u64) {
        FREE.lock().push(Free { pages, phys, virt });
    }
}

// SAFETY: HAL 方法只在驱动自己的用户态任务内调用；DMA 与 MMIO 地址都来自
// DeviceGrant 授权，映射由内核校验；共享缓冲在一次请求内保持有效。
unsafe impl Hal for HalImpl {
    fn dma_alloc(pages: usize, _direction: BufferDirection) -> (PhysAddr, NonNull<u8>) {
        let (phys, virt) = Self::alloc_pages(pages).expect("DMA 分配失败");
        // SAFETY: SYS_DMA_ALLOC 已把该连续区间映射到当前任务。
        unsafe {
            core::ptr::write_bytes(virt as *mut u8, 0, pages * PAGE);
        }
        (phys, NonNull::new(virt as *mut u8).expect("非空 DMA 指针"))
    }

    unsafe fn dma_dealloc(paddr: PhysAddr, vaddr: NonNull<u8>, pages: usize) -> i32 {
        HalImpl::release_pages(pages, paddr, vaddr.as_ptr() as u64);
        0
    }

    unsafe fn mmio_phys_to_virt(paddr: PhysAddr, size: usize) -> NonNull<u8> {
        let descriptor = *GRANT_DESC.lock();
        let Some(descriptor) = descriptor else {
            panic!("没有 DeviceGrant");
        };
        for (index, range) in descriptor.mmio.iter().enumerate() {
            let Some(range) = range else {
                continue;
            };
            if paddr < range.base || paddr.saturating_add(size as u64) > range.base + range.len {
                continue;
            }
            let grant = GRANT.load(Ordering::Acquire);
            let virt = mmio_virt(grant, &descriptor, index).expect("MMIO 映射失败");
            let pointer = virt + (paddr - range.base);
            return NonNull::new(pointer as *mut u8).expect("MMIO 指针");
        }
        panic!("MMIO 不在 DeviceGrant 范围");
    }

    unsafe fn share(buffer: NonNull<[u8]>, direction: BufferDirection) -> PhysAddr {
        // SAFETY: 调用方保证缓冲区在本任务内有效。
        let bytes = unsafe { buffer.as_ref() };
        let len = bytes.len();
        let address = bytes.as_ptr() as u64;
        if let Some(region) = FRAMES.lock().iter().find(|region| {
            address >= region.virt && address + len as u64 <= region.virt + region.len as u64
        }) {
            let phys = region.phys + (address - region.virt);
            let to_device = direction != BufferDirection::DeviceToDriver;
            let _ = dma_sync(GRANT.load(Ordering::Acquire), phys, len, to_device);
            SHARED.lock().push(Shared {
                phys,
                virt: 0,
                len,
                borrowed: true,
            });
            return phys;
        }
        let pages = len.div_ceil(PAGE);
        let (phys, virt) = Self::alloc_pages(pages).expect("共享缓冲分配失败");
        if direction != BufferDirection::DeviceToDriver {
            // SAFETY: DMA 区间已映射，来源缓冲区有效。
            unsafe {
                core::ptr::copy_nonoverlapping(bytes.as_ptr(), virt as *mut u8, len);
            }
        }
        SHARED.lock().push(Shared {
            phys,
            virt,
            len,
            borrowed: false,
        });
        let _ = dma_sync(GRANT.load(Ordering::Acquire), phys, len, true);
        phys
    }

    unsafe fn unshare(paddr: PhysAddr, buffer: NonNull<[u8]>, direction: BufferDirection) {
        let mut shared = SHARED.lock();
        let Some(index) = shared.iter().position(|entry| entry.phys == paddr) else {
            return;
        };
        let entry = shared.swap_remove(index);
        drop(shared);
        if !entry.borrowed && direction != BufferDirection::DriverToDevice {
            // SAFETY: 调用方保证 buffer 仍有效；长度来自 share 时的记录。
            unsafe {
                core::ptr::copy_nonoverlapping(
                    entry.virt as *const u8,
                    buffer.as_ptr() as *mut u8,
                    entry.len,
                );
            }
        }
        let _ = dma_sync(GRANT.load(Ordering::Acquire), entry.phys, entry.len, false);
        if !entry.borrowed {
            HalImpl::release_pages(entry.len.div_ceil(PAGE), entry.phys, entry.virt);
        }
    }
}

/// 返回指定 MMIO BAR 的自动映射地址；首次访问时映射并缓存。
pub(crate) fn mmio_virt(grant: u64, descriptor: &DeviceGrant, index: usize) -> Option<u64> {
    let mut mmio = MMIO.lock();
    if let Some(Some(virt)) = mmio.get(index) {
        return Some(*virt);
    }
    descriptor.mmio.get(index).copied().flatten()?;
    let virt = device_map(grant, index as u64)?;
    while mmio.len() <= index {
        mmio.push(None);
    }
    mmio[index] = Some(virt);
    Some(virt)
}

/// 通过系统调用访问 PCI 配置空间。
pub struct SysConfig;

impl ConfigurationAccess for SysConfig {
    fn read_word(&self, device: DeviceFunction, offset: u8) -> u32 {
        pci_read(device.bus, device.device, device.function, offset)
    }

    fn write_word(&mut self, device: DeviceFunction, offset: u8, data: u32) {
        pci_write(device.bus, device.device, device.function, offset, data);
    }

    unsafe fn unsafe_clone(&self) -> Self {
        Self
    }
}

impl hk_hal::PciConfig for SysConfig {
    fn read32(&self, address: hk_hal::PciAddress) -> u32 {
        pci_read(
            address.bus,
            address.device,
            address.function,
            address.offset,
        )
    }

    fn write32(&self, address: hk_hal::PciAddress, value: u32) {
        pci_write(
            address.bus,
            address.device,
            address.function,
            address.offset,
            value,
        );
    }
}
