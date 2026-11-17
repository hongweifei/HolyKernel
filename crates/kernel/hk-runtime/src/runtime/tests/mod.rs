//! 用户运行时单元测试。

use crate::runtime::*;
use alloc::string::String;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use hk_hal::{SyscallFrame, UserContext};

mod calls;
mod capacity;
mod device;
mod frame_map;
mod frames;
mod ipc;
mod ledger;
mod lifecycle;
mod payload;
mod provider;
mod reclaim;
mod region;
mod scanout;
mod services;
mod snapshot;
mod tasks;

/// 最小用户现场。
#[derive(Clone, Copy, Default)]
struct MockContext {
    rax: u64,
    rdi: u64,
    rsi: u64,
    rdx: u64,
    rcx: u64,
    r8: u64,
    r10: u64,
}

impl UserContext for MockContext {
    const SAVED_BYTES: usize = 7 * 8;

    fn resume(&self) {}

    fn save(&self, out: &mut [u8]) {
        let fields = [
            self.rax, self.rdi, self.rsi, self.rdx, self.rcx, self.r8, self.r10,
        ];
        for (index, field) in fields.iter().enumerate() {
            out[index * 8..index * 8 + 8].copy_from_slice(&field.to_le_bytes());
        }
    }

    fn load(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 7 * 8 {
            return None;
        }
        let mut fields = [0_u64; 7];
        for (index, field) in fields.iter_mut().enumerate() {
            *field = u64::from_le_bytes(bytes[index * 8..index * 8 + 8].try_into().ok()?);
        }
        Some(Self {
            rax: fields[0],
            rdi: fields[1],
            rsi: fields[2],
            rdx: fields[3],
            rcx: fields[4],
            r8: fields[5],
            r10: fields[6],
        })
    }

    fn set_start_argument(&mut self, argument: u64) {
        self.rdi = argument;
    }

    fn set_start_capacity(&mut self, capacity: u64) {
        self.rsi = capacity;
    }

    fn for_thread(entry: u64, _stack_top: u64, argument: u64) -> Self {
        Self {
            rax: entry,
            rdi: argument,
            ..Self::default()
        }
    }
}

impl SyscallFrame for MockContext {
    fn number(&self) -> u64 {
        self.rax
    }

    fn arg0(&self) -> u64 {
        self.rdi
    }

    fn arg1(&self) -> u64 {
        self.rsi
    }

    fn arg2(&self) -> u64 {
        self.rdx
    }

    fn arg3(&self) -> u64 {
        self.r10
    }

    fn set_result(&mut self, value: u64) {
        self.rax = value;
    }

    fn set_word(&mut self, value: u64) {
        self.rdx = value;
    }

    fn set_sender(&mut self, value: u64) {
        self.rcx = value;
    }

    fn set_handle(&mut self, value: u64) {
        self.r8 = value;
    }
}

struct MockPlatform {
    /// 物理页零检测的返回值。
    zero: bool,
    /// 物理页是否报告为跨地址空间共享。
    shared: bool,
    /// 解除映射的返回值。
    unmap_ok: Arc<AtomicBool>,
    /// 地址空间激活次数。
    activations: Arc<AtomicUsize>,
    /// 最近一次 `map_user` 的页表参数。
    last_map: Arc<core::sync::atomic::AtomicU64>,
    /// 共享的顶层 User Capacity。
    user: Arc<spin::Mutex<hk_mem::hmp::Capacity>>,
    /// 每槽位捕获的用户页，供检查点恢复测试。
    pages: Arc<spin::Mutex<alloc::collections::BTreeMap<usize, Vec<hk_checkpoint::Page>>>>,
    /// 解除映射调用次数。
    unmapped: Arc<AtomicUsize>,
    /// 释放地址空间调用次数。
    released: Arc<AtomicUsize>,
    /// COW 解析返回值；`None` 表示未处理。
    cow: Option<CowOutcome>,
    /// 容量 Ledger 枚举快照。
    ledgers: Vec<hk_mem::LedgerStats>,
    /// 最近一次用户态写入的数据。
    written: Arc<spin::Mutex<Vec<u8>>>,
    /// 最近一次 fork 传入的 COW 区间。
    last_cow: Arc<spin::Mutex<Vec<(u64, u64)>>>,
    /// 可选的记录型 IOMMU；`None` 表示平台没有 IOMMU。
    iommu: Option<RecordingIommu>,
    /// `map_user` 的返回值；用于构造页表重映射失败。
    map_ok: Arc<AtomicBool>,
    /// 供 `read_user` 读取的用户缓冲。
    input: Arc<spin::Mutex<Vec<u8>>>,
    /// 常驻物理内存内容，键为物理地址。
    phys: Arc<spin::Mutex<alloc::collections::BTreeMap<u64, u8>>>,
    /// 用户页是否已映射；默认 true 以保持既有测试语义。
    mapped: Arc<AtomicBool>,
    /// 物理页报告的 NUMA 节点。
    node: u8,
}

/// 只记录映射权限的软件 IOMMU，用于验证 DMA 映射从句柄派生 Rights。
struct RecordingIommu {
    maps: Arc<spin::Mutex<Vec<(hk_hal::Iova, hk_hal::IommuFlags)>>>,
}

impl hk_hal::Iommu for RecordingIommu {
    fn map(
        &self,
        _domain: hk_hal::IommuDomain,
        iova: hk_hal::Iova,
        _phys: hk_hal::PhysRange,
        flags: hk_hal::IommuFlags,
    ) -> Result<(), hk_hal::IommuError> {
        self.maps.lock().push((iova, flags));
        Ok(())
    }

    fn unmap(
        &self,
        _domain: hk_hal::IommuDomain,
        _iova: hk_hal::Iova,
        _len: usize,
    ) -> Result<(), hk_hal::IommuError> {
        Ok(())
    }

    fn flush(&self, _domain: hk_hal::IommuDomain) -> Result<(), hk_hal::IommuError> {
        Ok(())
    }

    fn translates_device_addresses(&self) -> bool {
        true
    }
}

impl MockPlatform {
    fn new() -> Self {
        let user = hk_mem::hmp::Capacity::new(
            hk_mem::hmp::CapacityId::next(),
            hk_hal::PhysRange::new(hk_hal::PhysAddr::new(0x9000_0000), 0x20_0000),
            hk_hal::PAGE_SIZE,
            hk_mem::hmp::CapacityKind::User,
            0,
        )
        .expect("User Capacity");
        Self {
            zero: true,
            shared: false,
            unmap_ok: Arc::new(AtomicBool::new(true)),
            activations: Arc::new(AtomicUsize::new(0)),
            last_map: Arc::new(core::sync::atomic::AtomicU64::new(0)),
            user: Arc::new(spin::Mutex::new(user)),
            pages: Arc::new(spin::Mutex::new(alloc::collections::BTreeMap::new())),
            unmapped: Arc::new(AtomicUsize::new(0)),
            released: Arc::new(AtomicUsize::new(0)),
            cow: None,
            ledgers: Vec::new(),
            written: Arc::new(spin::Mutex::new(Vec::new())),
            last_cow: Arc::new(spin::Mutex::new(Vec::new())),
            iommu: None,
            map_ok: Arc::new(AtomicBool::new(true)),
            input: Arc::new(spin::Mutex::new(Vec::new())),
            phys: Arc::new(spin::Mutex::new(alloc::collections::BTreeMap::new())),
            mapped: Arc::new(AtomicBool::new(true)),
            node: 0,
        }
    }
}

/// 测试平台的每任务内存配额。
const MEMORY_LIMIT: u64 = 1 << 20;

/// Arena 分配前由内核持有的常驻句柄数。
const ARENA_HANDLES: usize = 0;

impl UserPlatform for MockPlatform {
    fn timer_ticks(&self) -> u64 {
        7
    }

    fn pci_read(&self, _bus: u8, _device: u8, _function: u8, _offset: u8) -> u32 {
        0x1234
    }

    fn zero(&self, _address: u64, _length: usize) {}

    fn dma_window(&self, len: usize) -> Option<hk_hal::PhysRange> {
        Some(hk_hal::PhysRange::new(
            hk_hal::PhysAddr::new(0x1000_0000),
            len as u64,
        ))
    }

    fn reserve_capacity(
        &self,
        len: usize,
        kind: hk_mem::hmp::CapacityKind,
    ) -> Option<hk_mem::hmp::Capacity> {
        hk_mem::hmp::Capacity::new(
            hk_mem::hmp::CapacityId::next(),
            hk_hal::PhysRange::new(hk_hal::PhysAddr::new(0x9000_0000), len as u64),
            hk_hal::PAGE_SIZE,
            kind,
            0,
        )
        .ok()
    }

    fn user_capacity(&self) -> Option<Arc<spin::Mutex<hk_mem::hmp::Capacity>>> {
        Some(Arc::clone(&self.user))
    }

    fn map_region(&self, _slot: usize, mapping: &hk_mem::RegionMapping) -> bool {
        self.last_map.store(
            mapping.rights.bits() as u64
                | ((mapping.flags.bits() as u64) << 32)
                | ((mapping.policy.code() as u64) << 48),
            Ordering::Release,
        );
        self.map_ok.load(Ordering::Acquire)
    }

    fn unmap_user(&self, _slot: usize, _virt: u64, _len: usize) -> bool {
        self.unmapped
            .fetch_add(1, core::sync::atomic::Ordering::AcqRel);
        self.unmap_ok.load(Ordering::Acquire)
    }

    fn page_is_zero(&self, _phys: hk_hal::PhysAddr) -> Option<bool> {
        Some(self.zero)
    }

    fn page_shared(&self, _phys: hk_hal::PhysAddr) -> bool {
        self.shared
    }

    fn resolve_cow(
        &self,
        _address: u64,
        _capacity: Option<&alloc::sync::Arc<spin::Mutex<hk_mem::hmp::Capacity>>>,
    ) -> Option<CowOutcome> {
        self.cow
    }

    fn ledger_stats(&self) -> Option<Vec<hk_mem::LedgerStats>> {
        Some(self.ledgers.clone())
    }

    fn user_mapped(&self, _address: u64) -> bool {
        self.mapped.load(Ordering::Acquire)
    }

    fn page_node(&self, _phys: hk_hal::PhysAddr) -> Option<u8> {
        Some(self.node)
    }

    fn arena_layout(&self, slot: usize) -> (u64, u64) {
        (0x10_0000 + slot as u64 * 0x2_0000, 0x1_0000)
    }

    fn memory_limit(&self, _slot: usize) -> u64 {
        MEMORY_LIMIT
    }

    fn fork(
        &self,
        _parent: usize,
        _child: usize,
        cow_ranges: &[(u64, u64)],
        _capacity: Option<&alloc::sync::Arc<spin::Mutex<hk_mem::hmp::Capacity>>>,
    ) -> Option<u64> {
        *self.last_cow.lock() = cow_ranges.to_vec();
        Some(0)
    }

    fn share_vspace(&self, _parent: usize, _child: usize) -> bool {
        true
    }

    fn activate(&self, _slot: usize) {
        self.activations.fetch_add(1, Ordering::AcqRel);
    }

    fn write_user(&self, _address: u64, data: &[u8]) -> bool {
        *self.written.lock() = data.to_vec();
        true
    }

    fn read_user(&self, address: u64, buffer: &mut [u8]) -> bool {
        let input = self.input.lock();
        let start = address as usize;
        let Some(end) = start.checked_add(buffer.len()) else {
            return false;
        };
        if end > input.len() {
            return false;
        }
        buffer.copy_from_slice(&input[start..end]);
        true
    }

    fn write_phys(&self, phys: hk_hal::PhysAddr, data: &[u8]) -> bool {
        let mut memory = self.phys.lock();
        for (index, byte) in data.iter().enumerate() {
            memory.insert(phys.get() + index as u64, *byte);
        }
        true
    }

    fn read_phys(&self, phys: hk_hal::PhysAddr, buffer: &mut [u8]) -> bool {
        let memory = self.phys.lock();
        for (index, slot) in buffer.iter_mut().enumerate() {
            let Some(byte) = memory.get(&(phys.get() + index as u64)) else {
                return false;
            };
            *slot = *byte;
        }
        true
    }

    fn framebuffer(&self) -> Option<hk_hal::FramebufferInfo> {
        Some(hk_hal::FramebufferInfo {
            phys: hk_hal::PhysAddr::new(0x8000_0000),
            width: 2,
            height: 2,
            stride: 8,
            format: hk_hal::PixelFormat::Rgb32,
            cache: hk_hal::CachePolicy::WriteBack,
        })
    }

    fn iommu(&self) -> Option<&dyn hk_hal::Iommu> {
        self.iommu.as_ref().map(|iommu| iommu as &dyn hk_hal::Iommu)
    }

    fn map_framebuffer(&self, _slot: usize, _virt: u64) -> Option<u64> {
        Some(16)
    }

    fn capture_pages(&self, slot: usize) -> Option<Vec<hk_checkpoint::Page>> {
        Some(self.pages.lock().get(&slot).cloned().unwrap_or_default())
    }

    fn restore_pages(&self, slot: usize, pages: &[hk_checkpoint::Page]) -> bool {
        self.pages.lock().insert(slot, pages.to_vec());
        true
    }

    fn release_pages(&self, slot: usize) {
        self.released
            .fetch_add(1, core::sync::atomic::Ordering::AcqRel);
        self.pages.lock().remove(&slot);
    }
}

fn runtime_with(second: &'static str) -> UserRuntime<MockContext, MockPlatform> {
    runtime_on(second, MockPlatform::new())
}

fn runtime_on<P: UserPlatform>(second: &'static str, platform: P) -> UserRuntime<MockContext, P> {
    let second = if second == hk_syscall::MEMORY_SERVICE {
        crate::ServiceSpec::new(second).with_capacity(hk_mem::hmp::CapacityKind::User, 0x20_0000)
    } else {
        crate::ServiceSpec::new(second)
    };
    let mut runtime = crate::boot::user_runtime(
        &[crate::ServiceSpec::new("a"), second],
        platform,
        |_index, endpoint| MockContext {
            rdi: endpoint,
            ..MockContext::default()
        },
    )
    .expect("装配运行时");
    for index in 0..runtime.task_count() {
        runtime.attach(
            index,
            MockContext {
                rdi: index as u64 + 1,
                ..MockContext::default()
            },
        );
    }
    runtime
}

fn runtime() -> UserRuntime<MockContext, MockPlatform> {
    runtime_with("b")
}

/// 创建惰性匿名能力并映射到自动窗口，返回能力句柄编号。
fn create_lazy_region<P: UserPlatform>(
    runtime: &mut UserRuntime<MockContext, P>,
    context: &mut MockContext,
    length: u64,
) -> u64 {
    context.rax = SYS_CAPABILITY_CREATE_LAZY;
    context.rdi = 4;
    context.rsi = length;
    assert_eq!(runtime.dispatch(0, context), Switch::Continue);
    let capability = context.rax;
    assert_ne!(capability, u64::MAX);

    context.rax = SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = length;
    context.r10 = 0b11;
    assert_eq!(runtime.dispatch(0, context), Switch::Continue);
    assert_ne!(context.rax, u64::MAX);
    capability
}

/// 窗口数量受限的平台：`unmap_dma` 归还一个窗口，模拟池回收。
struct ShortPlatform {
    windows: core::sync::atomic::AtomicUsize,
}

impl UserPlatform for ShortPlatform {
    fn timer_ticks(&self) -> u64 {
        7
    }

    fn pci_read(&self, _bus: u8, _device: u8, _function: u8, _offset: u8) -> u32 {
        0x1234
    }

    fn zero(&self, _address: u64, _length: usize) {}

    fn dma_window(&self, len: usize) -> Option<hk_hal::PhysRange> {
        let available = self.windows.load(core::sync::atomic::Ordering::Acquire);
        if available == 0 {
            return None;
        }
        self.windows
            .fetch_sub(1, core::sync::atomic::Ordering::AcqRel);
        Some(hk_hal::PhysRange::new(
            hk_hal::PhysAddr::new(0x1000_0000),
            len as u64,
        ))
    }

    fn map_region(&self, _slot: usize, _mapping: &hk_mem::RegionMapping) -> bool {
        true
    }

    fn unmap_user(&self, _slot: usize, _virt: u64, _len: usize) -> bool {
        self.windows
            .fetch_add(1, core::sync::atomic::Ordering::AcqRel);
        true
    }

    fn page_is_zero(&self, _phys: hk_hal::PhysAddr) -> Option<bool> {
        Some(true)
    }

    fn user_mapped(&self, _address: u64) -> bool {
        true
    }

    fn arena_layout(&self, slot: usize) -> (u64, u64) {
        (0x10_0000 + slot as u64 * 0x2_0000, 0x1_0000)
    }

    fn reserve_capacity(
        &self,
        _len: usize,
        kind: hk_mem::hmp::CapacityKind,
    ) -> Option<hk_mem::hmp::Capacity> {
        hk_mem::hmp::Capacity::new(
            hk_mem::hmp::CapacityId::next(),
            hk_hal::PhysRange::new(hk_hal::PhysAddr::new(0xA000_0000), 0x3000),
            hk_hal::PAGE_SIZE,
            kind,
            0,
        )
        .ok()
    }

    fn user_capacity(&self) -> Option<Arc<spin::Mutex<hk_mem::hmp::Capacity>>> {
        Some(Arc::new(spin::Mutex::new(
            hk_mem::hmp::Capacity::new(
                hk_mem::hmp::CapacityId::next(),
                hk_hal::PhysRange::new(hk_hal::PhysAddr::new(0x9000_0000), 0x20_0000),
                hk_hal::PAGE_SIZE,
                hk_mem::hmp::CapacityKind::User,
                0,
            )
            .ok()?,
        )))
    }

    fn memory_limit(&self, _slot: usize) -> u64 {
        MEMORY_LIMIT
    }

    fn fork(
        &self,
        _parent: usize,
        _child: usize,
        _cow_ranges: &[(u64, u64)],
        _capacity: Option<&alloc::sync::Arc<spin::Mutex<hk_mem::hmp::Capacity>>>,
    ) -> Option<u64> {
        Some(0)
    }

    fn share_vspace(&self, _parent: usize, _child: usize) -> bool {
        true
    }

    fn write_user(&self, _address: u64, _data: &[u8]) -> bool {
        true
    }
}
