//! x86_64 的 HAL `Platform` 适配。
//!
//! 把既有的具体模块（串口、TSC/LAPIC 定时器、legacy PCI、DMA 池、direct map、
//! IOAPIC 与页表）包装成 [`hk_hal::Platform`] 的 trait 对象，作为多架构 HAL
//! 冻结的 x86_64 实现。启动路径仍复用现有模块，平台只提供统一入口。

use alloc::vec::Vec;

use hk_base::Moment;
use hk_hal::{
    Arch, BootInfo, Console, CpuId, DmaOps, Entropy, InterruptController, MAX_CPUS, MapFlags, Mmu,
    MmuError, PAGE_SIZE, PhysAddr, PhysicalAccess, PhysicalError, Platform, PlatformError, Timer,
    VSpaceId, VirtAddr,
};
use hk_test::{ExitCode, exit};
use spin::Mutex;

use crate::cpu;
use crate::paging::{self, PageFlags, PageTable};
use crate::port::{inb, outb};
use crate::serial::{COM1, Serial};

/// x86_64 平台。
#[derive(Clone, Copy, Debug, Default)]
pub struct X86Platform;

/// 全局平台实例。
pub static PLATFORM: X86Platform = X86Platform;

impl Platform for X86Platform {
    fn name(&self) -> &'static str {
        "x86_64-qemu"
    }

    fn arch(&self) -> Arch {
        Arch::X86_64
    }

    fn cpu_count(&self) -> usize {
        (1 + hk_smp::AP_POOL.online()).min(MAX_CPUS)
    }

    fn init(&self, boot: &BootInfo) -> Result<(), PlatformError> {
        boot.validate().map_err(PlatformError::BootInfo)
    }

    fn init_cpu(&self, cpu: CpuId) -> Result<(), PlatformError> {
        if cpu.get() as usize >= MAX_CPUS {
            return Err(PlatformError::CpuOutOfRange { cpu });
        }
        Ok(())
    }

    fn shutdown(&self) -> ! {
        if crate::bootstrap::power_off() {
            loop {
                x86_64::instructions::hlt();
            }
        }
        exit(ExitCode::Success)
    }

    fn reboot(&self) -> ! {
        if crate::bootstrap::reset() {
            loop {
                x86_64::instructions::hlt();
            }
        }
        // 8042 复位脉冲：等待输入缓冲为空后写入 reset 命令。
        // SAFETY: 0x64/0x60 是标准键盘控制器端口；复位成功后不会返回。
        unsafe {
            for _ in 0..100_000 {
                if inb(0x64) & 0x02 == 0 {
                    break;
                }
            }
            outb(0x64, 0xFE);
        }
        loop {
            x86_64::instructions::hlt();
        }
    }

    fn mmu(&self) -> &dyn Mmu {
        &MMU
    }

    fn irq(&self) -> &dyn InterruptController {
        &crate::bootstrap::CONTROLLER
    }

    fn timer(&self) -> &dyn Timer {
        &TIMER
    }

    fn console(&self) -> &dyn Console {
        &CONSOLE
    }

    fn pci(&self) -> Option<&dyn hk_device::PciConfig> {
        Some(&crate::pci::PCI)
    }

    fn dma(&self) -> &dyn DmaOps {
        &crate::dma::DMA
    }

    fn physical(&self) -> &dyn PhysicalAccess {
        &PHYSICAL
    }

    fn entropy(&self) -> &dyn Entropy {
        &crate::entropy::ENTROPY
    }

    fn iommu(&self) -> Option<&dyn hk_hal::Iommu> {
        Some(&crate::iommu::IOMMU)
    }

    fn cache(&self) -> Option<&dyn hk_hal::CacheOps> {
        Some(&crate::cache::CACHE)
    }
}

/// 串口控制台。
pub struct X86Console;

static CONSOLE: X86Console = X86Console;

impl Console for X86Console {
    fn write_str(&self, text: &str) {
        Serial::new(COM1).write_str(text);
    }
}

/// TSC 单调时钟与 LAPIC 一次性定时器。
pub struct X86Timer;

static TIMER: X86Timer = X86Timer;

impl Timer for X86Timer {
    fn frequency(&self) -> u64 {
        let tsc = crate::timing::tsc_frequency();
        if tsc != 0 {
            tsc
        } else {
            crate::hpet::frequency()
        }
    }

    fn now(&self) -> Moment {
        if crate::timing::tsc_frequency() != 0 {
            Moment::from_ticks(crate::timing::rdtsc())
        } else {
            Moment::from_ticks(crate::hpet::now())
        }
    }

    fn set_oneshot(&self, deadline: Moment) {
        if crate::timing::tsc_frequency() == 0 {
            return;
        }
        let frequency = self.frequency();
        if frequency == 0 {
            return;
        }
        let now = crate::timing::rdtsc();
        let delta = deadline.ticks().saturating_sub(now);
        let micros = (u128::from(delta) * 1_000_000 / u128::from(frequency)) as u64;
        cpu::arm_oneshot(micros);
    }

    fn cancel(&self) {
        cpu::disarm_timer();
    }
}

/// direct map 物理内存访问。
pub struct X86Physical;

static PHYSICAL: X86Physical = X86Physical;

impl PhysicalAccess for X86Physical {
    fn read(&self, address: PhysAddr, buffer: &mut [u8]) -> Result<(), PhysicalError> {
        let base = paging::direct_map_base();
        if base == 0 {
            return Err(PhysicalError::OutOfRange);
        }
        let source = (base + address.get()) as *const u8;
        // SAFETY: 调用方保证地址落在 direct map 覆盖的物理内存内。
        unsafe { core::ptr::copy_nonoverlapping(source, buffer.as_mut_ptr(), buffer.len()) };
        Ok(())
    }

    fn write(&self, address: PhysAddr, bytes: &[u8]) -> Result<(), PhysicalError> {
        let base = paging::direct_map_base();
        if base == 0 {
            return Err(PhysicalError::OutOfRange);
        }
        let target = (base + address.get()) as *mut u8;
        // SAFETY: 调用方保证地址落在 direct map 覆盖的物理内存内。
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len()) };
        Ok(())
    }

    fn copy(&self, from: PhysAddr, to: PhysAddr, len: usize) -> Result<(), PhysicalError> {
        let base = paging::direct_map_base();
        if base == 0 {
            return Err(PhysicalError::OutOfRange);
        }
        let source = (base + from.get()) as *const u8;
        let target = (base + to.get()) as *mut u8;
        // SAFETY: 调用方保证两端都落在 direct map 覆盖的物理内存内且不重叠。
        unsafe { core::ptr::copy_nonoverlapping(source, target, len) };
        Ok(())
    }
}

/// 用户地址空间注册表。
pub struct X86Mmu {
    tables: Mutex<Vec<Option<PageTable>>>,
}

static MMU: X86Mmu = X86Mmu::new();

impl X86Mmu {
    /// 构造空地址空间表。
    pub const fn new() -> Self {
        Self {
            tables: Mutex::new(Vec::new()),
        }
    }

    fn index(vspace: VSpaceId) -> Option<usize> {
        usize::try_from(vspace.get().checked_sub(1)?).ok()
    }
}

impl Default for X86Mmu {
    fn default() -> Self {
        Self::new()
    }
}

impl Mmu for X86Mmu {
    fn page_size(&self) -> usize {
        PAGE_SIZE
    }

    fn create_vspace(&self) -> VSpaceId {
        let table = paging::kernel_table()
            .create_user(&paging::FRAME_BANK)
            .expect("创建用户页表");
        let mut tables = self.tables.lock();
        if let Some(index) = tables.iter().position(Option::is_none) {
            tables[index] = Some(table);
            VSpaceId::new(index as u64 + 1)
        } else {
            let index = tables.len();
            tables.push(Some(table));
            VSpaceId::new(index as u64 + 1)
        }
    }

    fn destroy_vspace(&self, vspace: VSpaceId) {
        let Some(index) = Self::index(vspace) else {
            return;
        };
        if let Some(table) = self.tables.lock().get_mut(index) {
            // 帧回收由运行时通过 UserVspace::release 负责；HAL 只解除登记。
            *table = None;
        }
    }

    fn map(
        &self,
        vspace: VSpaceId,
        frame: PhysAddr,
        virt: VirtAddr,
        len: usize,
        flags: MapFlags,
    ) -> Result<(), MmuError> {
        if len == 0
            || !len.is_multiple_of(PAGE_SIZE)
            || !frame.is_page_aligned()
            || !virt.is_page_aligned()
        {
            return Err(MmuError::Unaligned);
        }
        if flags.violates_wx() {
            return Err(MmuError::WxConflict);
        }
        let page_flags = PageFlags::from_map(flags);
        let Some(index) = Self::index(vspace) else {
            return Err(MmuError::InvalidAddressSpace);
        };
        let mut tables = self.tables.lock();
        let Some(table) = tables.get_mut(index).and_then(|slot| slot.as_ref()) else {
            return Err(MmuError::InvalidAddressSpace);
        };
        let mut offset = 0;
        while offset < len {
            if flags.contains(MapFlags::GUARD) {
                offset += PAGE_SIZE;
                continue;
            }
            table
                .map_mmio(
                    VirtAddr::new(virt.get() + offset as u64),
                    PhysAddr::new(frame.get() + offset as u64),
                    page_flags,
                )
                .map_err(|error| match error {
                    paging::PagingError::OutOfFrames => MmuError::OutOfMemory,
                    _ => MmuError::AlreadyMapped,
                })?;
            offset += PAGE_SIZE;
        }
        drop(tables);
        if !flags.contains(MapFlags::GUARD) {
            cpu::shootdown_tlb(None);
        }
        Ok(())
    }

    fn unmap(&self, vspace: VSpaceId, virt: VirtAddr, len: usize) -> Result<PhysAddr, MmuError> {
        if len == 0 || !len.is_multiple_of(PAGE_SIZE) || !virt.is_page_aligned() {
            return Err(MmuError::Unaligned);
        }
        let Some(index) = Self::index(vspace) else {
            return Err(MmuError::InvalidAddressSpace);
        };
        let mut tables = self.tables.lock();
        let Some(table) = tables.get_mut(index).and_then(|slot| slot.as_ref()) else {
            return Err(MmuError::InvalidAddressSpace);
        };
        let physical = table
            .translate(virt)
            .map(|(phys, _)| phys)
            .ok_or(MmuError::NotMapped)?;
        if !table.unmap_user_range(virt, len) {
            return Err(MmuError::NotMapped);
        }
        drop(tables);
        cpu::shootdown_tlb(None);
        Ok(physical)
    }

    fn protect(
        &self,
        vspace: VSpaceId,
        virt: VirtAddr,
        len: usize,
        flags: MapFlags,
    ) -> Result<(), MmuError> {
        if len == 0 || !len.is_multiple_of(PAGE_SIZE) || !virt.is_page_aligned() {
            return Err(MmuError::Unaligned);
        }
        if flags.violates_wx() {
            return Err(MmuError::WxConflict);
        }
        let Some(index) = Self::index(vspace) else {
            return Err(MmuError::InvalidAddressSpace);
        };
        let mut tables = self.tables.lock();
        let Some(table) = tables.get_mut(index).and_then(|slot| slot.as_ref()) else {
            return Err(MmuError::InvalidAddressSpace);
        };
        if !table.protect_user_range(virt, len, PageFlags::from_map(flags)) {
            return Err(MmuError::NotMapped);
        }
        drop(tables);
        cpu::shootdown_tlb(None);
        Ok(())
    }

    fn translate(&self, vspace: VSpaceId, virt: VirtAddr) -> Option<(PhysAddr, MapFlags)> {
        let index = Self::index(vspace)?;
        let tables = self.tables.lock();
        let table = tables.get(index)?.as_ref()?;
        let (phys, raw) = table.translate(virt)?;
        Some((phys, paging::map_flags(raw)))
    }

    fn shootdown(&self, _vspace: VSpaceId) {
        cpu::shootdown_tlb(None);
    }
}
