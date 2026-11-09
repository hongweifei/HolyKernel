//! Intel VT-d 硬件后端。
//!
//! 使用 `intel-iommu` 的寄存器/上下文结构与第二级页表；每个 BDF 域拥有独立
//! 的第二级页表，映射/解除映射后按全局粒度失效上下文缓存与 IOTLB。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use hk_hal::{Iommu, IommuDomain, IommuError, IommuFlags, Iova, PhysAddr, PhysRange};
use hk_mem::{FrameBank, FrameKind};
use intel_iommu::context::{AddrWidth, ContextEntry, RootEntry, TransType};
use intel_iommu::regs::{
    CAP, CCMD, Cap, Ccmd, Cirg, ECAP, Ecap, GCMD, GSTS, Gcmd, Gsts, Iirg, IotlbInval, RTADDR,
    Rtaddr, iotlb_offset,
};
use spin::Mutex;

use super::second_stage::{PAGE_SIZE, SecondStage, TableMemory};

const INVALIDATE_LIMIT: usize = 1_000_000;

/// 用物理帧银行构建 VT-d 表。
pub struct PlatformMemory {
    direct_map: u64,
    bank: &'static FrameBank,
}

impl PlatformMemory {
    pub(super) fn new(direct_map: u64) -> Self {
        Self {
            direct_map,
            bank: &crate::paging::FRAME_BANK,
        }
    }

    pub(super) fn allocate_pages(&mut self, len: usize) -> Option<u64> {
        let range = self.bank.allocate_contiguous_for(FrameKind::Other, len)?;
        let start = range.start().get();
        let pointer = self.direct_map.saturating_add(start) as *mut u8;
        // SAFETY: 连续物理区间来自帧银行，direct map 覆盖该区域。
        unsafe {
            core::ptr::write_bytes(pointer, 0, len);
        }
        Some(start)
    }

    fn zero(&self, physical: u64) {
        let pointer = self.direct_map.saturating_add(physical) as *mut u8;
        // SAFETY: 物理页来自帧银行，direct map 覆盖所有可用物理内存。
        unsafe {
            core::ptr::write_bytes(pointer, 0, PAGE_SIZE as usize);
        }
    }

    fn address(&self, physical: u64) -> *mut u64 {
        self.direct_map.saturating_add(physical) as *mut u64
    }
}

impl TableMemory for PlatformMemory {
    fn allocate_table(&mut self) -> Option<u64> {
        let frame = self.bank.allocate_for(FrameKind::Other)?;
        self.zero(frame.get());
        Some(frame.get())
    }

    fn read(&self, physical: u64) -> u64 {
        // SAFETY: 只有 VT-d 表页会走这个接口，物理地址来自帧银行。
        unsafe { self.address(physical).read_volatile() }
    }

    fn write(&mut self, physical: u64, value: u64) {
        // SAFETY: 同上；写入的是表项。
        unsafe { self.address(physical).write_volatile(value) };
    }
}

struct Unit {
    base: u64,
    iotlb: u64,
}

impl Unit {
    fn new(base: u64) -> Option<Self> {
        let ecap = Ecap(read64(base + ECAP));
        if Cap(read64(base + CAP)).sagaw() & 4 == 0 {
            return None;
        }
        Some(Self {
            base,
            iotlb: base + iotlb_offset(ecap).1,
        })
    }

    fn enable(&self, root: u64) -> bool {
        write64(self.base + RTADDR, Rtaddr(root).0);
        write32(self.base + GCMD, Gcmd::SRTP.bits());
        if !wait32(self.base + GSTS, Gsts::RTPS.bits()) {
            return false;
        }
        write32(self.base + GCMD, Gcmd::ENABLE.bits());
        wait32(self.base + GSTS, Gsts::TES.bits())
    }

    fn disable(&self) {
        write32(self.base + GCMD, 0);
    }

    fn invalidate_context(&self) {
        write64(
            self.base + CCMD,
            Ccmd::new(Cirg::Global, 0, 0, 0).0 | (1 << 63),
        );
        wait64_clear(self.base + CCMD, 1 << 63);
    }

    fn invalidate_iotlb(&self) {
        write64(
            self.iotlb,
            IotlbInval::new(Iirg::Global, 0, true, true).0 | (1 << 63),
        );
        wait64_clear(self.iotlb, 1 << 63);
    }
}

struct Domain {
    table: SecondStage,
    entries: BTreeMap<u64, Entry>,
}

#[derive(Clone, Copy)]
struct Entry {
    physical: u64,
    len: u64,
    flags: IommuFlags,
}

struct State {
    memory: PlatformMemory,
    root: u64,
    contexts: BTreeMap<u16, u64>,
    domains: BTreeMap<u16, Domain>,
    units: Vec<Unit>,
}

impl State {
    fn ensure_domain(&mut self, id: u16) -> Result<bool, IommuError> {
        if self.domains.contains_key(&id) {
            return Ok(false);
        }
        let bus = id >> 8;
        let devfn = id & 0xFF;
        let context = if let Some(context) = self.contexts.get(&bus) {
            *context
        } else {
            let context = self
                .memory
                .allocate_table()
                .ok_or(IommuError::OutOfMemory)?;
            let root_entry = RootEntry::new(context);
            self.memory
                .write(self.root + u64::from(bus) * 16, root_entry.low);
            self.memory
                .write(self.root + u64::from(bus) * 16 + 8, root_entry.high);
            self.contexts.insert(bus, context);
            context
        };
        let table = SecondStage::new(&mut self.memory).ok_or(IommuError::OutOfMemory)?;
        let entry = ContextEntry::new()
            .with_present(true)
            .with_translation_type(TransType::SecondStage)
            .with_address_width(AddrWidth::Agaw48)
            .with_second_stage_ptr(table.root())
            .with_domain_id(id);
        self.memory.write(context + u64::from(devfn) * 16, entry.q0);
        self.memory
            .write(context + u64::from(devfn) * 16 + 8, entry.q1);
        self.domains.insert(
            id,
            Domain {
                table,
                entries: BTreeMap::new(),
            },
        );
        Ok(true)
    }

    fn invalidate_context(&self) {
        for unit in &self.units {
            unit.invalidate_context();
        }
    }

    fn invalidate_iotlb(&self) {
        for unit in &self.units {
            unit.invalidate_iotlb();
        }
    }
}

/// VT-d 后端。
pub struct VtdIommu {
    state: Mutex<State>,
}

impl VtdIommu {
    /// 映射寄存器页并启用所有可用的 DRHD 单元。
    pub fn new(units: &[u64], direct_map: u64) -> Option<Self> {
        let mut memory = PlatformMemory::new(direct_map);
        let root = memory.allocate_table()?;
        let mut enabled: Vec<Unit> = Vec::new();
        for &base in units {
            let Some(unit) = Unit::new(base) else {
                continue;
            };
            if !unit.enable(root) {
                unit.disable();
                for unit in &enabled {
                    unit.disable();
                }
                return None;
            }
            enabled.push(unit);
        }
        if enabled.is_empty() {
            return None;
        }
        Some(Self {
            state: Mutex::new(State {
                memory,
                root,
                contexts: BTreeMap::new(),
                domains: BTreeMap::new(),
                units: enabled,
            }),
        })
    }
}

impl Iommu for VtdIommu {
    fn map(
        &self,
        domain: IommuDomain,
        iova: Iova,
        phys: PhysRange,
        flags: IommuFlags,
    ) -> Result<(), IommuError> {
        let id = u16::try_from(domain.get()).map_err(|_| IommuError::Unsupported)?;
        let start = iova.get();
        let len = phys.len();
        let mut state = self.state.lock();
        if state.ensure_domain(id)? {
            state.invalidate_context();
        }
        {
            let State {
                memory, domains, ..
            } = &mut *state;
            let Some(domain) = domains.get_mut(&id) else {
                return Err(IommuError::Unsupported);
            };
            let end = start.checked_add(len).ok_or(IommuError::OutOfMemory)?;
            if domain.entries.iter().any(|(slot_start, entry)| {
                let slot_end = slot_start.saturating_add(entry.len);
                *slot_start < end && start < slot_end
            }) {
                return Err(IommuError::AlreadyMapped);
            }
            domain
                .table
                .map(memory, start, phys.start().get(), len, flags)?;
            domain.entries.insert(
                start,
                Entry {
                    physical: phys.start().get(),
                    len,
                    flags,
                },
            );
        }
        state.invalidate_iotlb();
        Ok(())
    }

    fn unmap(&self, domain: IommuDomain, iova: Iova, len: usize) -> Result<(), IommuError> {
        let id = u16::try_from(domain.get()).map_err(|_| IommuError::Unsupported)?;
        let mut state = self.state.lock();
        {
            let State {
                memory, domains, ..
            } = &mut *state;
            let Some(domain) = domains.get_mut(&id) else {
                return Err(IommuError::NotMapped);
            };
            let Some(entry) = domain.entries.get(&iova.get()).copied() else {
                return Err(IommuError::NotMapped);
            };
            if entry.len != len as u64 {
                return Err(IommuError::Unaligned);
            }
            domain.table.unmap(memory, iova.get(), entry.len)?;
            domain.entries.remove(&iova.get());
        }
        state.invalidate_iotlb();
        Ok(())
    }

    fn flush(&self, _domain: IommuDomain) -> Result<(), IommuError> {
        self.state.lock().invalidate_iotlb();
        Ok(())
    }

    fn translate(&self, domain: IommuDomain, iova: Iova) -> Option<(PhysRange, IommuFlags)> {
        let id = u16::try_from(domain.get()).ok()?;
        let state = self.state.lock();
        let domain = state.domains.get(&id)?;
        let start = iova.get();
        let (slot, entry) = domain
            .entries
            .range(..=start)
            .next_back()
            .filter(|(slot, entry)| start < slot.saturating_add(entry.len))?;
        let offset = start - *slot;
        Some((
            PhysRange::new(PhysAddr::new(entry.physical + offset), entry.len - offset),
            entry.flags,
        ))
    }

    fn translates_device_addresses(&self) -> bool {
        true
    }
}

fn read64(base: u64) -> u64 {
    // SAFETY: register base 由启动期映射为设备页。
    unsafe { (base as *const u64).read_volatile() }
}

fn write64(base: u64, value: u64) {
    // SAFETY: 同上。
    unsafe { (base as *mut u64).write_volatile(value) };
}

fn write32(base: u64, value: u32) {
    // SAFETY: 同上。
    unsafe { (base as *mut u32).write_volatile(value) };
}

fn wait32(base: u64, bits: u32) -> bool {
    for _ in 0..INVALIDATE_LIMIT {
        // SAFETY: 同上。
        let value = unsafe { (base as *const u32).read_volatile() };
        if value & bits == bits {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

fn wait64_clear(base: u64, bits: u64) {
    for _ in 0..INVALIDATE_LIMIT {
        if read64(base) & bits == 0 {
            return;
        }
        core::hint::spin_loop();
    }
}
