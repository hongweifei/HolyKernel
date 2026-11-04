//! x86_64 引导期初始化：帧缓冲控制台、LAPIC、ACPI MADT、IOAPIC 与键盘。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use hk_hal::{BootInfo, PAGE_SIZE, PhysAddr, VirtAddr};

use crate::ioapic::IoApic;
use crate::{FrameAllocator, FramebufferConsole, PageFlags, PageTable};

/// I/O APIC 与键盘路由结果。
pub struct IoApicSetup {
    /// 键盘 IRQ 对应的全局系统中断。
    pub gsi: u32,
    /// 键盘中断向量。
    pub vector: u8,
    /// 目标本地 APIC 编号。
    pub destination: u8,
    /// MADT 报告的 CPU 数量。
    pub cpus: usize,
}

/// ACPI 表的临时映射窗口，避开 HHDM 未覆盖的物理页。
const ACPI_SCRATCH_BASE: u64 = 0xFFFF_9000_0000_0000;

/// 表窗口可容纳的最大页数。
const ACPI_MAX_PAGES: usize = 16;

struct AcpiWindow<'a> {
    paging: &'a PageTable,
    allocator: &'a mut FrameAllocator,
    base: u64,
}

impl AcpiWindow<'_> {
    fn read(&mut self, phys: u64, length: usize) -> Option<Vec<u8>> {
        if length == 0 {
            return None;
        }
        let page = PAGE_SIZE as u64;
        let offset = (phys & (page - 1)) as usize;
        let span = offset.checked_add(length)?;
        let pages = span.div_ceil(PAGE_SIZE);
        if pages > ACPI_MAX_PAGES {
            return None;
        }

        let page_phys = phys & !(page - 1);
        for index in 0..pages {
            self.paging
                .map(
                    VirtAddr::new(self.base + index as u64 * page),
                    PhysAddr::new(page_phys + index as u64 * page),
                    PageFlags::KERNEL_RW,
                    self.allocator,
                )
                .ok()?;
        }

        let mut bytes = alloc::vec![0_u8; length];
        let source = (self.base + offset as u64) as *const u8;
        // SAFETY: 刚建立的映射覆盖 `offset + length` 字节，目标缓冲长度相同。
        unsafe {
            core::ptr::copy_nonoverlapping(source, bytes.as_mut_ptr(), length);
        }
        Some(bytes)
    }
}

/// 解析 MADT，映射 IOAPIC 并把键盘 GSI 路由到键盘向量。
pub fn init_ioapic(
    info: &BootInfo,
    paging: &PageTable,
    allocator: &mut FrameAllocator,
    direct_map_base: u64,
) -> Result<Option<IoApicSetup>, String> {
    let Some(rsdp) = info.rsdp else {
        return Ok(None);
    };

    let mut window = AcpiWindow {
        paging,
        allocator,
        base: ACPI_SCRATCH_BASE,
    };
    let madt = crate::acpi::parse(rsdp.get(), |phys, length| window.read(phys, length));

    let Some(madt) = madt else {
        return Ok(None);
    };

    let phys = madt.ioapic_base;
    let virt = VirtAddr::new(direct_map_base + phys);
    paging
        .map(virt, PhysAddr::new(phys), PageFlags::MMIO, allocator)
        .map_err(|error| format!("映射 IOAPIC 失败：{error:?}"))?;

    crate::keyboard::init();

    let ioapic = IoApic::new(virt.get());
    ioapic.init();
    let gsi = madt.gsi(1);
    let vector = crate::cpu::KEYBOARD_VECTOR as u8;
    let destination = crate::cpu::lapic_id();
    ioapic.route(gsi, madt.gsi_base, vector, destination);

    Ok(Some(IoApicSetup {
        gsi,
        vector,
        destination,
        cpus: madt.cpu_count(),
    }))
}

/// 以启动信息中的帧缓冲描述创建文本控制台。
pub fn framebuffer_console(info: &BootInfo) -> Option<FramebufferConsole> {
    FramebufferConsole::from_boot_info(info)
}

/// 映射 LAPIC 寄存器页并初始化本地 APIC。
pub fn init_interrupts(
    paging: &PageTable,
    allocator: &mut FrameAllocator,
    direct_map_base: u64,
) -> Result<(), String> {
    let lapic_phys = PhysAddr::new(0xFEE0_0000);
    let lapic_virt = VirtAddr::new(direct_map_base + lapic_phys.get());
    paging
        .map(lapic_virt, lapic_phys, PageFlags::MMIO, allocator)
        .map_err(|error| format!("映射 LAPIC 失败：{error:?}"))?;

    crate::cpu::init_apic(lapic_virt.get());
    Ok(())
}
