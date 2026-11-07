//! x86_64 引导期初始化：帧缓冲控制台、LAPIC、ACPI MADT、IOAPIC 与键盘。

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use hk_hal::{
    BootInfo, CpuId, InterruptController, Ipi, Irq, IrqError, NotificationId, PAGE_SIZE, PhysAddr,
    VirtAddr,
};
use spin::Mutex;

use crate::acpi::Madt;
use crate::ioapic::IoApic;
use crate::{FramebufferConsole, PageAllocator, PageFlags, PageTable};

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

struct IoApicState {
    base: u64,
    destination: u8,
}

static MADT: Mutex<Option<Madt>> = Mutex::new(None);
static IOAPIC: Mutex<Option<IoApicState>> = Mutex::new(None);
static IRQ_BINDINGS: Mutex<BTreeMap<u32, Vec<NotificationId>>> = Mutex::new(BTreeMap::new());

/// x86 IOAPIC 中断控制器。
pub struct IoApicController;

/// 全局 x86 中断控制器。
pub static CONTROLLER: IoApicController = IoApicController;

/// 屏蔽 IRQ 线。
pub fn unroute_irq(irq: u8) -> bool {
    let Some(madt) = *MADT.lock() else {
        return false;
    };
    let ioapic = IOAPIC.lock();
    let Some(state) = ioapic.as_ref() else {
        return false;
    };
    IoApic::new(state.base).mask(madt.gsi(irq), madt.gsi_base);
    true
}

/// 把 IRQ 线路由到对应的设备中断向量。
pub fn route_irq(irq: u8) -> bool {
    if irq >= crate::irq::IRQ_LINES {
        return false;
    }
    let Some(madt) = *MADT.lock() else {
        return false;
    };
    let ioapic = IOAPIC.lock();
    let Some(state) = ioapic.as_ref() else {
        return false;
    };
    let vector = crate::irq::IRQ_VECTOR_BASE + irq;
    IoApic::new(state.base).route_level(madt.gsi(irq), madt.gsi_base, vector, state.destination);
    true
}

impl InterruptController for IoApicController {
    fn bind(&self, irq: Irq, target: NotificationId) -> Result<(), IrqError> {
        let line = irq.get();
        if line >= u32::from(crate::irq::IRQ_LINES) {
            return Err(IrqError::Unsupported);
        }
        let mut bindings = IRQ_BINDINGS.lock();
        if let Some(targets) = bindings.get_mut(&line) {
            targets.push(target);
            return Ok(());
        }
        if !route_irq(line as u8) {
            return Err(IrqError::Unsupported);
        }
        bindings.insert(line, alloc::vec![target]);
        Ok(())
    }

    fn bind_direct(&self, irq: Irq, target: NotificationId) -> Result<(), IrqError> {
        let line = irq.get();
        if line >= u32::from(crate::irq::IRQ_LINES) {
            return Err(IrqError::Unsupported);
        }
        IRQ_BINDINGS.lock().entry(line).or_default().push(target);
        Ok(())
    }

    fn unbind(&self, irq: Irq) -> Result<(), IrqError> {
        let line = irq.get();
        if IRQ_BINDINGS.lock().remove(&line).is_none() {
            return Err(IrqError::NotBound);
        }
        unroute_irq(line as u8);
        Ok(())
    }

    fn mask(&self, irq: Irq) {
        unroute_irq(irq.get() as u8);
    }

    fn unmask(&self, irq: Irq) {
        route_irq(irq.get() as u8);
    }

    fn eoi(&self, _irq: Irq) {
        // LAPIC EOI 由中断分派路径负责。
    }

    fn send_ipi(&self, cpu: CpuId, ipi: Ipi) {
        let vector = match ipi {
            Ipi::Halt => return,
            Ipi::Reschedule | Ipi::TlbShootdown => crate::cpu::IPI_VECTOR,
        };
        crate::cpu::send_ipi(cpu.get() as u8, vector);
    }
}

struct AcpiWindow<'a> {
    paging: &'a PageTable,
    allocator: &'a PageAllocator,
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
    allocator: &PageAllocator,
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
    let vector = crate::board::KEYBOARD_VECTOR as u8;
    let destination = crate::cpu::lapic_id();
    ioapic.route(gsi, madt.gsi_base, vector, destination);
    *MADT.lock() = Some(madt);
    *IOAPIC.lock() = Some(IoApicState {
        base: virt.get(),
        destination,
    });

    Ok(Some(IoApicSetup {
        gsi,
        vector,
        destination,
        cpus: madt.cpu_count(),
    }))
}

/// 初始化键盘并把 IRQ1 路由到键盘向量。
pub fn init_keyboard(setup: &IoApicSetup) -> bool {
    crate::keyboard::init();
    let Some(madt) = *MADT.lock() else {
        return false;
    };
    let ioapic = IOAPIC.lock();
    let Some(state) = ioapic.as_ref() else {
        return false;
    };
    IoApic::new(state.base).route(madt.gsi(1), madt.gsi_base, setup.vector, setup.destination);
    true
}

/// 以启动信息中的帧缓冲描述创建文本控制台。
pub fn framebuffer_console(info: &BootInfo) -> Option<FramebufferConsole> {
    FramebufferConsole::from_boot_info(info)
}

/// 映射 LAPIC 寄存器页并初始化本地 APIC。
pub fn init_interrupts(
    paging: &PageTable,
    allocator: &PageAllocator,
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
