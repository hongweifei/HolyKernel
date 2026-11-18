//! AMD-Vi 硬件后端。
//!
//! 使用 IVRS 的 IVHD 单元、`amd-iommu` 的设备表项/命令缓冲结构与 v1 主机页表；
//! 每个 BDF 域对应一个 DTE，映射变更通过全局失效命令提交。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use amd_iommu::cmd::{Command, CompletionWait};
use amd_iommu::dte::{DeviceTableEntry, PagingMode};
use amd_iommu::regs::{
    BufferBase, CMD_BUF_BASE, CMD_HEAD, CMD_TAIL, CONTROL, Control, DEV_TABLE_BASE,
    DeviceTableBase, EVT_BUF_BASE,
};
use hk_hal::{Iommu, IommuDomain, IommuError, IommuFlags, Iova, PhysAddr, PhysRange};
use spin::Mutex;

use super::amd_second_stage::V1PageTable;
use super::second_stage::TableMemory;
use super::vtd::PlatformMemory;

const COMMAND_BYTES: u64 = 4096;
const DEVICE_TABLE_BYTES: usize = 2 * 1024 * 1024;
const DEVICE_TABLE_SIZE_CODE: u16 = 511;
const WAIT_LIMIT: usize = 1_000_000;

struct Unit {
    base: u64,
}

impl Unit {
    fn enable(&self, device_table: u64, command: u64) -> bool {
        write64(
            self.base + DEV_TABLE_BASE,
            DeviceTableBase::new(device_table, DEVICE_TABLE_SIZE_CODE).bits(),
        );
        write64(self.base + CMD_BUF_BASE, BufferBase::new(command, 8).bits());
        write64(self.base + EVT_BUF_BASE, 0);
        write64(self.base + CMD_HEAD, 0);
        write64(self.base + CMD_TAIL, 0);
        write64(
            self.base + CONTROL,
            (Control::CMD_BUF_EN | Control::IOMMU_EN | Control::COHERENT_EN).bits(),
        );
        read64(self.base + CONTROL) & (Control::CMD_BUF_EN | Control::IOMMU_EN).bits()
            == (Control::CMD_BUF_EN | Control::IOMMU_EN).bits()
    }

    fn disable(&self) {
        write64(self.base + CONTROL, 0);
    }
}

#[derive(Clone, Copy)]
struct Entry {
    physical: u64,
    len: u64,
    flags: IommuFlags,
}

struct Domain {
    table: V1PageTable,
    entries: BTreeMap<u64, Entry>,
}

struct State {
    memory: PlatformMemory,
    device_table: u64,
    command: u64,
    semaphore: u64,
    tail: u64,
    generation: u64,
    domains: BTreeMap<u16, Domain>,
    units: Vec<Unit>,
}

impl State {
    fn ensure_domain(&mut self, id: u16) -> Result<bool, IommuError> {
        if self.domains.contains_key(&id) {
            return Ok(false);
        }
        let table = V1PageTable::new(&mut self.memory).ok_or(IommuError::OutOfMemory)?;
        let entry = DeviceTableEntry::new()
            .with_valid(true)
            .with_translation_valid(true)
            .with_paging_mode(PagingMode::Level4)
            .with_host_page_table_root(table.root())
            .with_domain_id(id)
            .with_io_coherent(true)
            .with_ir(true)
            .with_iw(true);
        let base = self.device_table + u64::from(id) * 32;
        self.memory.write(base, entry.q0);
        self.memory.write(base + 8, entry.q1);
        self.memory.write(base + 16, entry.q2);
        self.memory.write(base + 24, entry.q3);
        self.domains.insert(
            id,
            Domain {
                table,
                entries: BTreeMap::new(),
            },
        );
        Ok(true)
    }

    fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let mut tail = self.tail;
        let first = Command::InvalidateIommuAll.words();
        write_command(&mut self.memory, self.command + tail, first);
        tail = (tail + 16) % COMMAND_BYTES;
        let wait =
            Command::CompletionWait(CompletionWait::new().with_store(self.semaphore, generation))
                .words();
        write_command(&mut self.memory, self.command + tail, wait);
        tail = (tail + 16) % COMMAND_BYTES;
        self.tail = tail;
        for unit in &self.units {
            write64(unit.base + CMD_TAIL, tail);
        }
        for _ in 0..WAIT_LIMIT {
            if self.memory.read(self.semaphore) == generation {
                return;
            }
            core::hint::spin_loop();
        }
    }
}

/// AMD-Vi 后端。
pub struct AmdIommu {
    state: Mutex<State>,
}

impl AmdIommu {
    /// 映射寄存器页并启用所有可用的 IVHD 单元。
    pub fn new(units: &[u64], direct_map: u64) -> Option<Self> {
        let mut memory = PlatformMemory::new(direct_map);
        let device_table = memory.allocate_pages(DEVICE_TABLE_BYTES)?;
        let command = memory.allocate_pages(COMMAND_BYTES as usize)?;
        let semaphore = memory.allocate_pages(4096)?;
        let mut enabled: Vec<Unit> = Vec::new();
        for &base in units {
            let unit = Unit { base };
            if !unit.enable(device_table, command) {
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
                device_table,
                command,
                semaphore,
                tail: 0,
                generation: 0,
                domains: BTreeMap::new(),
                units: enabled,
            }),
        })
    }
}

impl Iommu for AmdIommu {
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
        state.ensure_domain(id)?;
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
        state.invalidate();
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
        state.invalidate();
        Ok(())
    }

    fn flush(&self, _domain: IommuDomain) -> Result<(), IommuError> {
        self.state.lock().invalidate();
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

fn write_command(memory: &mut PlatformMemory, address: u64, words: [u32; 4]) {
    memory.write(address, u64::from(words[0]) | (u64::from(words[1]) << 32));
    memory.write(
        address + 8,
        u64::from(words[2]) | (u64::from(words[3]) << 32),
    );
}

fn read64(base: u64) -> u64 {
    // SAFETY: 寄存器页在启动期已映射为设备页。
    unsafe { (base as *const u64).read_volatile() }
}

fn write64(base: u64, value: u64) {
    // SAFETY: 同上。
    unsafe { (base as *mut u64).write_volatile(value) };
}
