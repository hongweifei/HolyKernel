//! virtio-blk 用户态适配（HAL + PCI 探测）。

use alloc::vec::Vec;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU64, Ordering};

use hk_hal::DeviceGrant;
use hk_server_api::{device_map, dma_alloc, dma_sync, pci_read, pci_write, write};
use spin::Mutex;
use virtio_drivers::device::blk::{SECTOR_SIZE, VirtIOBlk};
use virtio_drivers::transport::pci::PciTransport;
use virtio_drivers::transport::pci::bus::{ConfigurationAccess, DeviceFunction, PciRoot};
use virtio_drivers::{BufferDirection, Hal, PhysAddr};

const PAGE: usize = 4096;
const DMA_BASE: u64 = 0x5000_0000;
const MMIO_BASE: u64 = 0x6000_0000;

static GRANT: AtomicU64 = AtomicU64::new(0);
static NEXT_DMA: AtomicU64 = AtomicU64::new(DMA_BASE);
static GRANT_DESC: Mutex<Option<DeviceGrant>> = Mutex::new(None);
static SHARED: Mutex<Vec<Shared>> = Mutex::new(Vec::new());

struct Shared {
    phys: u64,
    virt: u64,
    len: usize,
}

struct HalImpl;

impl HalImpl {
    fn alloc_pages(pages: usize) -> Option<(u64, u64)> {
        let len = pages.checked_mul(PAGE)?;
        let virt = NEXT_DMA.fetch_add(len as u64, Ordering::AcqRel);
        let grant = GRANT.load(Ordering::Acquire);
        if grant == 0 {
            return None;
        }
        let phys = dma_alloc(grant, len, virt);
        (phys != 0).then_some((phys, virt))
    }
}

unsafe impl Hal for HalImpl {
    fn dma_alloc(pages: usize, _direction: BufferDirection) -> (PhysAddr, NonNull<u8>) {
        write(b"virtio: dma\n");
        let (phys, virt) = Self::alloc_pages(pages).expect("DMA 分配失败");
        write(b"virtio: dma ok\n");
        // SAFETY: SYS_DMA_ALLOC 已把该连续区间映射到当前任务。
        unsafe {
            core::ptr::write_bytes(virt as *mut u8, 0, pages * PAGE);
        }
        (phys, NonNull::new(virt as *mut u8).expect("非空 DMA 指针"))
    }

    unsafe fn dma_dealloc(_paddr: PhysAddr, _vaddr: NonNull<u8>, _pages: usize) -> i32 {
        0
    }

    unsafe fn mmio_phys_to_virt(paddr: PhysAddr, size: usize) -> NonNull<u8> {
        write(b"virtio: mmio\n");
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
            let result = device_map(grant, index as u64, virt);
            write(b"virtio: mmio mapped\n");
            let _ = result;
            let pointer = virt + (paddr - range.base);
            return NonNull::new(pointer as *mut u8).expect("MMIO 指针");
        }
        panic!("MMIO 不在 DeviceGrant 范围");
    }

    unsafe fn share(buffer: NonNull<[u8]>, direction: BufferDirection) -> PhysAddr {
        let bytes = unsafe { buffer.as_ref() };
        let len = bytes.len();
        let pages = len.div_ceil(PAGE);
        let (phys, virt) = Self::alloc_pages(pages).expect("共享缓冲分配失败");
        if direction != BufferDirection::DeviceToDriver {
            // SAFETY: DMA 区间已映射，来源缓冲区有效。
            unsafe {
                core::ptr::copy_nonoverlapping(bytes.as_ptr(), virt as *mut u8, len);
            }
        }
        SHARED.lock().push(Shared { phys, virt, len });
        let _ = dma_sync(GRANT.load(Ordering::Acquire), phys, len, true);
        phys
    }

    unsafe fn unshare(paddr: PhysAddr, buffer: NonNull<[u8]>, direction: BufferDirection) {
        let mut shared = SHARED.lock();
        let Some(index) = shared.iter().position(|entry| entry.phys == paddr) else {
            return;
        };
        let entry = shared.swap_remove(index);
        if direction != BufferDirection::DriverToDevice {
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
    }
}

struct SysConfig;

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

/// 尝试初始化 virtio-blk；没有设备或初始化失败时返回 `None`。
fn try_init(
    grant_handle: u64,
    descriptor: DeviceGrant,
) -> Option<VirtIOBlk<HalImpl, PciTransport>> {
    GRANT.store(grant_handle, Ordering::Release);
    *GRANT_DESC.lock() = Some(descriptor);
    write(b"virtio: descriptor\n");
    let mut root = PciRoot::new(SysConfig);
    let function = root
        .enumerate_bus(0)
        .find(|(_, info)| {
            info.vendor_id == 0x1AF4 && (info.device_id == 0x1001 || info.device_id == 0x1042)
        })
        .map(|(function, _)| function)?;
    write(b"virtio: transport\n");
    let transport = PciTransport::new::<HalImpl, _>(&mut root, function).ok()?;
    write(b"virtio: blk\n");
    let block = VirtIOBlk::new(transport).ok()?;
    write(b"hk-server-block: virtio-blk ready\n");
    Some(block)
}

pub struct VirtioBlock {
    block: spin::Mutex<VirtIOBlk<HalImpl, PciTransport>>,
}

impl VirtioBlock {
    /// 尝试构造。
    pub fn new(grant_handle: u64, descriptor: DeviceGrant) -> Option<Self> {
        Self::try_init(grant_handle, descriptor).map(|block| Self {
            block: spin::Mutex::new(block),
        })
    }

    fn try_init(
        grant_handle: u64,
        descriptor: DeviceGrant,
    ) -> Option<VirtIOBlk<HalImpl, PciTransport>> {
        try_init(grant_handle, descriptor)
    }
}

impl hk_block::BlockDevice for VirtioBlock {
    fn block_size(&self) -> usize {
        SECTOR_SIZE
    }

    fn capacity_blocks(&self) -> u64 {
        self.block.lock().capacity()
    }

    fn read_blocks(&self, block: u64, buffer: &mut [u8]) -> Result<usize, hk_block::BlockError> {
        self.block
            .lock()
            .read_blocks(block as usize, buffer)
            .map_err(|_| hk_block::BlockError::Io)?;
        Ok(buffer.len())
    }

    fn write_blocks(&mut self, block: u64, data: &[u8]) -> Result<usize, hk_block::BlockError> {
        self.block
            .lock()
            .write_blocks(block as usize, data)
            .map_err(|_| hk_block::BlockError::Io)?;
        Ok(data.len())
    }

    fn flush(&mut self) -> Result<(), hk_block::BlockError> {
        self.block
            .lock()
            .flush()
            .map_err(|_| hk_block::BlockError::Io)
    }
}
