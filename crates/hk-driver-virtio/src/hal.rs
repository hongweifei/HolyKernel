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
/// DMA 虚拟基址。
const DMA_BASE: u64 = 0x5000_0000;
/// MMIO 虚拟基址。
const MMIO_BASE: u64 = 0x6000_0000;

static GRANT: AtomicU64 = AtomicU64::new(0);
static NEXT_DMA: AtomicU64 = AtomicU64::new(DMA_BASE);
static GRANT_DESC: Mutex<Option<DeviceGrant>> = Mutex::new(None);
static SHARED: Mutex<Vec<Shared>> = Mutex::new(Vec::new());
static FREE: Mutex<Vec<Free>> = Mutex::new(Vec::new());
static FRAMES: Mutex<Vec<FrameRegion>> = Mutex::new(Vec::new());

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

/// 登记共享 Frame 的用户虚拟地址与物理区间，供 DMA 直接引用。
pub fn register_frame(virt: u64, phys: u64, len: usize) {
    FRAMES.lock().push(FrameRegion { virt, phys, len });
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
        let virt = NEXT_DMA.fetch_add(len as u64, Ordering::AcqRel);
        let grant = GRANT.load(Ordering::Acquire);
        if grant == 0 {
            return None;
        }
        let phys = dma_alloc(grant, len, virt);
        (phys != 0).then_some((phys, virt))
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
            let virt = MMIO_BASE + index as u64 * 0x1_0000;
            let grant = GRANT.load(Ordering::Acquire);
            let _ = device_map(grant, index as u64, virt);
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
