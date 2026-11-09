//! x86_64 引导期初始化：帧缓冲控制台、LAPIC、ACPI MADT、IOAPIC 与键盘。

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use hk_hal::{
    BootInfo, CpuId, InterruptController, Ipi, Irq, IrqError, NotificationId, PAGE_SIZE, PhysAddr,
    VirtAddr,
};
use spin::Mutex;

use crate::ioapic::IoApic;
use crate::{FramebufferConsole, PageAllocator, PageFlags, PageTable};
use hk_fw::FirmwareInfo;
use hk_fw::acpi::Madt;
use hk_fw::acpi::tables::Fadt;
use hk_fw::acpi::tables::McfgEntry;

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

/// MCFG ECAM 的常驻内核虚拟窗口。
const ECAM_BASE: u64 = 0xFFFF_C000_0000_0000;

/// 每个 MCFG 窗口占用的虚拟地址步长（1 GiB）。
const ECAM_STRIDE: u64 = 1 << 30;

/// HPET 寄存器页的常驻内核虚拟窗口。
const HPET_BASE: u64 = 0xFFFF_D000_0000_0000;

/// 表窗口可容纳的最大页数（1 MiB，覆盖大型 SRAT/PPTT）。
const ACPI_MAX_PAGES: usize = 256;

struct IoApicState {
    base: u64,
    destination: u8,
}

static MADT: Mutex<Option<Madt>> = Mutex::new(None);
static POWER: Mutex<Option<Fadt>> = Mutex::new(None);
static S5_TYPES: Mutex<Option<(u8, u8)>> = Mutex::new(None);
static GPE_EVENTS: AtomicU64 = AtomicU64::new(0);
/// 已订阅 ACPI 电源/睡眠事件的用户监听者数量。
static ACPI_LISTENERS: AtomicUsize = AtomicUsize::new(0);
static FIRMWARE: Mutex<Option<hk_fw::acpi::tables::Tables>> = Mutex::new(None);
static FIRMWARE_INFO: Mutex<Option<FirmwareInfo>> = Mutex::new(None);
static IOAPIC: Mutex<Option<IoApicState>> = Mutex::new(None);
static IRQ_BINDINGS: Mutex<BTreeMap<u32, Vec<NotificationId>>> = Mutex::new(BTreeMap::new());

/// 每条 IRQ 的目标本地 APIC 编号；0 表示尚未指定。
static IRQ_AFFINITY: Mutex<[u8; 256]> = Mutex::new([0; 256]);

/// 选择 CPU 索引：按 IRQ 线均摊到已启动 CPU。
fn affinity_destination(line: u32) -> Option<u8> {
    let count = crate::cpu::online_cpu_count();
    if count == 0 {
        return crate::cpu::apic_id(0);
    }
    crate::cpu::online_apic_id(line as usize % count)
}

/// x86 IOAPIC 中断控制器。
pub struct IoApicController;

/// 全局 x86 中断控制器。
pub static CONTROLLER: IoApicController = IoApicController;

/// 屏蔽 IRQ 线。
pub fn unroute_irq(irq: u8) -> bool {
    let Some(madt) = MADT.lock().as_ref().cloned() else {
        return false;
    };
    let ioapic = IOAPIC.lock();
    let Some(state) = ioapic.as_ref() else {
        return false;
    };
    IoApic::new(state.base).mask(madt.gsi(irq), madt.gsi_base());
    true
}

/// 把 IRQ 线路由到对应的设备中断向量。
pub fn route_irq(irq: u8) -> bool {
    if irq >= crate::irq::IRQ_LINES {
        return false;
    }
    let Some(madt) = MADT.lock().as_ref().cloned() else {
        return false;
    };
    let ioapic = IOAPIC.lock();
    let Some(state) = ioapic.as_ref() else {
        return false;
    };
    let vector = crate::irq::IRQ_VECTOR_BASE + irq;
    let destination = {
        let affinity = IRQ_AFFINITY.lock();
        let saved = affinity[irq as usize];
        if saved != 0 { saved } else { state.destination }
    };
    IoApic::new(state.base).route_level(madt.gsi(irq), madt.gsi_base(), vector, destination);
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
        if let Some(destination) = affinity_destination(line) {
            IRQ_AFFINITY.lock()[line as usize] = destination;
            route_irq(line as u8);
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

    fn set_affinity(&self, irq: Irq, cpu: CpuId) {
        let line = irq.get();
        let Some(destination) = crate::cpu::apic_id(cpu.get() as usize) else {
            return;
        };
        if line >= IRQ_AFFINITY.lock().len() as u32 {
            return;
        }
        IRQ_AFFINITY.lock()[line as usize] = destination;
        if IRQ_BINDINGS.lock().contains_key(&line) {
            route_irq(line as u8);
        }
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
    let tables = hk_fw::acpi::tables::parse(rsdp.get(), |phys, length| window.read(phys, length));
    crate::iommu::init(&tables, paging, allocator);
    *POWER.lock() = tables.fadt;
    if let Some(mcfg) = &tables.mcfg {
        for (index, entry) in mcfg.entries.iter().enumerate() {
            map_ecam(
                paging,
                allocator,
                entry,
                ECAM_BASE + index as u64 * ECAM_STRIDE,
            );
        }
    }
    if let Some(hpet) = &tables.hpet
        && hpet.address.space == 0
        && hpet.address.address != 0
    {
        let offset = hpet.address.address & 0xFFF;
        let phys = hpet.address.address & !0xFFF;
        if paging
            .map(
                VirtAddr::new(HPET_BASE),
                PhysAddr::new(phys),
                PageFlags::MMIO,
                allocator,
            )
            .is_ok()
        {
            crate::hpet::init(HPET_BASE + offset);
            if crate::hpet::available() {
                if crate::timing::tsc_frequency() == 0
                    && let Some(hz) = crate::timing::calibrate_with_hpet()
                {
                    crate::rtc::init(hz);
                }
                crate::cpu::calibrate_tick_hpet();
            }
        }
    }
    let madt = hk_fw::acpi::parse(rsdp.get(), |phys, length| window.read(phys, length));
    *FIRMWARE_INFO.lock() = Some(tables.firmware_info(madt.as_ref()));
    *FIRMWARE.lock() = Some(tables);
    if let Some(fadt) = *POWER.lock() {
        let mut types = None;
        if fadt.dsdt != 0
            && let Some(dsdt) =
                hk_fw::acpi::tables::read_table(fadt.dsdt, |phys, length| window.read(phys, length))
        {
            types = hk_fw::acpi::tables::parse_s5(&dsdt);
        }
        if types.is_none() {
            for ssdt in hk_fw::acpi::tables::find_tables(rsdp.get(), b"SSDT", |phys, length| {
                window.read(phys, length)
            }) {
                if let Some(found) = hk_fw::acpi::tables::parse_s5(&ssdt) {
                    types = Some(found);
                    break;
                }
            }
        }
        *S5_TYPES.lock() = types;
    }
    let Some(madt) = madt else {
        return Ok(None);
    };
    let Some(phys) = madt.ioapic_base() else {
        return Ok(None);
    };
    let virt = VirtAddr::new(direct_map_base + phys);
    paging
        .map(virt, PhysAddr::new(phys), PageFlags::MMIO, allocator)
        .map_err(|error| format!("映射 IOAPIC 失败：{error:?}"))?;

    crate::keyboard::init();

    let ioapic = IoApic::new(virt.get());
    ioapic.init();
    let gsi = madt.gsi(1);
    let gsi_base = madt.gsi_base();
    let cpus = madt.cpu_count();
    let vector = crate::board::KEYBOARD_VECTOR as u8;
    let destination = crate::cpu::lapic_id();
    ioapic.route(gsi, gsi_base, vector, destination);
    *MADT.lock() = Some(madt);
    *IOAPIC.lock() = Some(IoApicState {
        base: virt.get(),
        destination,
    });
    if let Some(fadt) = *POWER.lock()
        && fadt.sci_interrupt != 0
    {
        ioapic.route_level(
            u32::from(fadt.sci_interrupt),
            gsi_base,
            crate::cpu::SCI_VECTOR as u8,
            destination,
        );
    }
    if let Some(gsi) = crate::hpet::start_periodic(gsi_base) {
        ioapic.route_level(gsi, gsi_base, crate::cpu::HPET_VECTOR as u8, destination);
    }

    Ok(Some(IoApicSetup {
        gsi,
        vector,
        destination,
        cpus,
    }))
}

/// 把 MCFG 描述的 ECAM 窗口映射到常驻内核虚拟地址。
fn map_ecam(paging: &PageTable, allocator: &PageAllocator, entry: &McfgEntry, virt_base: u64) {
    let length = entry.length();
    let mut offset = 0_u64;
    while offset < length {
        let virt = virt_base + offset;
        let phys = entry.base + offset;
        let remaining = length - offset;
        let mapped = if remaining >= (1 << 21)
            && virt.is_multiple_of(1 << 21)
            && phys.is_multiple_of(1 << 21)
        {
            paging
                .map_huge(
                    VirtAddr::new(virt),
                    PhysAddr::new(phys),
                    PageFlags::MMIO,
                    1 << 21,
                    allocator,
                )
                .is_ok()
        } else {
            paging
                .map(
                    VirtAddr::new(virt),
                    PhysAddr::new(phys),
                    PageFlags::MMIO,
                    allocator,
                )
                .is_ok()
        };
        if !mapped {
            return;
        }
        offset += if remaining >= (1 << 21)
            && virt.is_multiple_of(1 << 21)
            && phys.is_multiple_of(1 << 21)
        {
            1 << 21
        } else {
            PAGE_SIZE as u64
        };
    }
    crate::pci::init_ecam(crate::pci::Ecam {
        base: virt_base,
        segment: entry.segment,
        bus_start: entry.bus_start,
        bus_end: entry.bus_end,
    });
}

/// 返回启动期解析到的 ACPI 固定表与拓扑表。
pub fn firmware() -> Option<hk_fw::acpi::tables::Tables> {
    FIRMWARE.lock().clone()
}

/// 返回启动期解析到的规范化固件描述。
pub fn firmware_info() -> Option<FirmwareInfo> {
    FIRMWARE_INFO.lock().clone()
}

/// 通过 FADT PM1 控制块进入 S5 软关机；缺少 FADT 时返回 `false`。
pub fn power_off() -> bool {
    let Some(fadt) = *POWER.lock() else {
        return false;
    };
    if fadt.pm1a_control == 0 {
        return false;
    }
    // 优先使用 DSDT `_S5`；缺失时使用 QEMU/常见固件的 5。
    let (a, b) = S5_TYPES.lock().unwrap_or((5, 5));
    let value_a = (u16::from(a & 0x07) << 10) | (1 << 13);
    let value_b = (u16::from(b & 0x07) << 10) | (1 << 13);
    write_pm1(fadt.pm1a_control, value_a, fadt.pm1_control_length);
    if fadt.pm1b_control != 0 {
        write_pm1(fadt.pm1b_control, value_b, fadt.pm1_control_length);
    }
    true
}

/// 登记一个用户态 ACPI 事件监听者。
pub fn set_acpi_listener() {
    ACPI_LISTENERS.fetch_add(1, Ordering::AcqRel);
}

/// 返回逻辑 CPU 的 ACPI/SRAT/PPTT 拓扑。
pub fn cpu_topology(cpu: usize) -> Option<hk_hal::CpuTopology> {
    let apic = u32::from(crate::cpu::apic_id(cpu)?);
    let madt = MADT.lock().as_ref().cloned()?;
    let acpi_id = madt.processor_for_apic(apic)?.acpi_id;
    let tables = FIRMWARE.lock().clone();
    let proximity = tables
        .as_ref()
        .and_then(|tables| tables.srat.as_ref())
        .and_then(|srat| srat.cpus.iter().find(|entry| entry.apic_id == apic))
        .map_or(0, |entry| entry.proximity);
    let package_id = tables
        .as_ref()
        .and_then(|tables| tables.pptt.as_ref())
        .and_then(|pptt| {
            pptt.processors
                .iter()
                .find(|entry| entry.processor_id == acpi_id)
        })
        .map_or(0, |entry| entry.package_id);
    Some(hk_hal::CpuTopology {
        acpi_id,
        package_id,
        proximity,
    })
}

/// 取出并清除待处理的 GPE 事件位图。
pub fn take_acpi_events() -> u64 {
    GPE_EVENTS.swap(0, Ordering::AcqRel)
}

/// 处理 ACPI SCI：清除 PM1/GPE 状态，电源按钮触发 S5。
pub fn handle_sci() {
    let Some(fadt) = *POWER.lock() else {
        return;
    };
    let mut power_button = false;
    let mut sleep_button = false;
    for (block, length) in [
        (fadt.pm1a_event, fadt.pm1_event_length),
        (fadt.pm1b_event, fadt.pm1_event_length),
    ] {
        if block == 0 || length == 0 {
            continue;
        }
        let status = read_pm1(block, length);
        if status != 0 {
            write_pm1(block, status, length);
        }
        power_button |= status & (1 << 8) != 0;
        sleep_button |= status & (1 << 9) != 0;
    }
    if fadt.gpe0_block != 0 && fadt.gpe0_length != 0 {
        for index in 0..u16::from(fadt.gpe0_length) {
            let port = (fadt.gpe0_block as u16).wrapping_add(index);
            // SAFETY: GPE0 块由 FADT 描述为标准 I/O 端口。
            let value = unsafe { crate::port::inb(port) };
            if value != 0 {
                if index < 8 {
                    GPE_EVENTS.fetch_or(u64::from(value) << (index * 8), Ordering::AcqRel);
                }
                // SAFETY: 写 1 清除对应 GPE 状态。
                unsafe { crate::port::outb(port, value) };
            }
        }
    }
    if fadt.gpe1_block != 0 && fadt.gpe1_length != 0 {
        let base = u16::from(fadt.gpe1_base);
        for index in 0..u16::from(fadt.gpe1_length) {
            let port = (fadt.gpe1_block as u16).wrapping_add(index);
            // SAFETY: GPE1 块由 FADT 描述为标准 I/O 端口。
            let value = unsafe { crate::port::inb(port) };
            if value != 0 {
                let bit = u32::from(base) + u32::from(index) * 8;
                if bit < 64 {
                    GPE_EVENTS.fetch_or(u64::from(value) << bit, Ordering::AcqRel);
                }
                // SAFETY: 写 1 清除对应 GPE 状态。
                unsafe { crate::port::outb(port, value) };
            }
        }
    }
    if GPE_EVENTS.load(Ordering::Acquire) != 0 {
        crate::irq::note(hk_hal::irq::ACPI_SCI_IRQ);
    }
    if power_button {
        crate::irq::note(hk_hal::irq::ACPI_POWER_IRQ);
    }
    if sleep_button {
        crate::irq::note(hk_hal::irq::ACPI_SLEEP_IRQ);
    }
    // 没有用户监听者时保留默认策略：电源按钮直接进入 S5。
    if power_button && ACPI_LISTENERS.load(Ordering::Acquire) == 0 && power_off() {
        loop {
            x86_64::instructions::hlt();
        }
    }
}

fn read_pm1(port: u32, length: u8) -> u16 {
    // SAFETY: PM1 事件块由 FADT 描述为标准 I/O 端口。
    unsafe {
        if length <= 1 {
            u16::from(crate::port::inb(port as u16))
        } else {
            crate::port::inw(port as u16)
        }
    }
}

/// 通过 FADT reset register 复位；无该寄存器时返回 `false`。
pub fn reset() -> bool {
    let Some(fadt) = *POWER.lock() else {
        return false;
    };
    if fadt.reset_register.address == 0 || fadt.reset_register.space != 1 {
        return false;
    }
    // SAFETY: FADT 报告的是标准复位 I/O 端口，宽度通常为 1 字节。
    unsafe {
        crate::port::outb(fadt.reset_register.address as u16, fadt.reset_value);
    }
    true
}

fn write_pm1(port: u32, value: u16, length: u8) {
    // SAFETY: FADT 报告的 PM1 控制块是标准 I/O 端口。
    unsafe {
        if length <= 1 {
            crate::port::outb(port as u16, value as u8);
        } else {
            crate::port::outw(port as u16, value);
        }
    }
}

/// 初始化键盘并把 IRQ1 路由到键盘向量。
pub fn init_keyboard(setup: &IoApicSetup) -> bool {
    crate::keyboard::init();
    let Some(madt) = MADT.lock().as_ref().cloned() else {
        return false;
    };
    let ioapic = IOAPIC.lock();
    let Some(state) = ioapic.as_ref() else {
        return false;
    };
    IoApic::new(state.base).route(
        madt.gsi(1),
        madt.gsi_base(),
        setup.vector,
        setup.destination,
    );
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
