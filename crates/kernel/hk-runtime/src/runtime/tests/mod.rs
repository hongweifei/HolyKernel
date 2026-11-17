//! 用户运行时单元测试。

use crate::runtime::*;
use alloc::string::String;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use hk_hal::{SyscallFrame, UserContext};

mod calls;
mod capacity;
mod ipc;
mod ledger;
mod payload;
mod provider;
mod reclaim;
mod region;
mod scanout;
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

#[test]
fn renders_cadence_meter_and_tempo_views() {
    let runtime = runtime();
    let cadence = runtime.cadence_text("a");
    assert!(cadence.contains("cadence a"), "{cadence}");
    assert!(cadence.contains("weight="), "{cadence}");
    let meter = runtime.meter_text("a");
    assert!(meter.contains("meter a"), "{meter}");
    assert!(meter.contains("consumed="), "{meter}");
    assert!(runtime.cadence_text("missing").contains("未知任务"));
    assert!(runtime.tempo_text().contains("Tempo 未运行"));
}

#[test]
fn checkpoint_restore_preserves_cadence_and_meter() {
    let mut runtime = runtime();
    let info = runtime
        .tasks()
        .into_iter()
        .find(|task| task.name == "a")
        .expect("任务 a");
    let tcb = hk_kernel::TcbId::new(info.tcb);
    let task = runtime.kernel.task_of(tcb).expect("Task");
    runtime
        .kernel
        .set_policy_authority(task, true)
        .expect("策略权限");
    let rights = hk_handle::Rights::READ | hk_handle::Rights::WRITE | hk_handle::Rights::ADMIN;
    let cadence = runtime
        .kernel
        .grant_cadence(tcb, task, rights)
        .expect("授予 Cadence");
    let mut spec = runtime.kernel.cadence_spec(task).expect("契约");
    spec.weight = 2048;
    spec.latency = hk_cadence::millis(2);
    runtime
        .kernel
        .update_cadence(tcb, cadence, spec)
        .expect("更新契约");
    runtime.kernel.charge(tcb, 1_234);

    let before = runtime.kernel.task_cadence_snapshot(tcb).expect("契约快照");
    let before_meter = runtime.kernel.task_meter_snapshot(tcb).expect("计量快照");

    let slot = runtime
        .tasks()
        .iter()
        .position(|task| task.name == "a")
        .expect("任务槽");
    runtime.checkpoint("a").expect("保存检查点");
    let restored = runtime.restore_checkpoint("a").expect("恢复检查点");
    assert_eq!(restored, slot, "应恢复到同一任务槽");

    let info = runtime
        .tasks()
        .into_iter()
        .find(|task| task.name == "a" && task.state != "exited")
        .expect("恢复后的任务");
    let tcb = hk_kernel::TcbId::new(info.tcb);
    let after = runtime.kernel.task_cadence_snapshot(tcb).expect("契约快照");
    let after_meter = runtime.kernel.task_meter_snapshot(tcb).expect("计量快照");

    assert_eq!(after.class, before.class);
    assert_eq!(after.weight, before.weight);
    assert_eq!(after.budget, before.budget);
    assert_eq!(after.period, before.period);
    assert_eq!(after.deadline, before.deadline);
    assert_eq!(after.latency, before.latency);
    assert_eq!(after_meter.consumed, before_meter.consumed);
    assert_eq!(after_meter.waited, before_meter.waited);
    assert_eq!(after_meter.throttled, before_meter.throttled);
    assert_eq!(after_meter.switches, before_meter.switches);
    assert_eq!(after_meter.overruns, before_meter.overruns);
    assert_eq!(after_meter.migrations, before_meter.migrations);
    assert_eq!(after_meter.migration_cost, before_meter.migration_cost);
}

#[test]
fn checkpoint_preserves_region_offset() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = SYS_CAPABILITY_CREATE_LAZY;
    context.rdi = 4;
    context.rsi = 0x2000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;
    assert_ne!(capability, u64::MAX);

    context.rax = SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0x1000;
    context.rdx = 0x1000;
    context.r10 = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_ne!(context.rax, u64::MAX);

    let offset = runtime
        .process(index)
        .regions
        .iter()
        .flatten()
        .next()
        .map(|region| region.offset);
    assert_eq!(offset, Some(0x1000));

    // 检查点要求任务不在任何 CPU 上执行。
    runtime.current[0] = None;
    runtime.checkpoint("a").expect("保存检查点");
    runtime.restore_checkpoint("a").expect("恢复检查点");

    let restored = runtime
        .process(index)
        .regions
        .iter()
        .flatten()
        .next()
        .map(|region| region.offset);
    assert_eq!(restored, Some(0x1000), "恢复必须保留 Region 的能力内偏移");
    let _ = runtime.finish(0, index);
}

#[test]
fn restart_grants_device_service_its_dma_capacity() {
    let mut runtime = runtime();
    let name = "hk-device-restart";
    let slot = runtime.reuse_slot(name);
    runtime
        .start_at_with_regions(
            slot,
            name,
            true,
            hk_cadence::DEFAULT_SPEC,
            MockContext::default(),
            None,
            &[],
        )
        .expect("启动设备服务");
    let process = runtime.process(slot);
    let tcb = runtime.tasks[slot].tcb;
    let (_, kind, _, limit, _) = runtime
        .kernel
        .capacity_info(tcb, process.allocation_capacity)
        .expect("容量信息");
    assert_eq!(kind, hk_mem::hmp::CapacityKind::Dma);
    assert_eq!(limit, crate::DEVICE_CAPACITY_BYTES as u64);

    runtime.terminate(name, 0);
    let slot = runtime.reuse_slot(name);
    runtime
        .start_at_with_regions(
            slot,
            name,
            true,
            hk_cadence::DEFAULT_SPEC,
            MockContext::default(),
            None,
            &[],
        )
        .expect("重启设备服务");
    let process = runtime.process(slot);
    let tcb = runtime.tasks[slot].tcb;
    let (_, kind, _, limit, _) = runtime
        .kernel
        .capacity_info(tcb, process.allocation_capacity)
        .expect("重启后的容量信息");
    assert_eq!(kind, hk_mem::hmp::CapacityKind::Dma);
    assert_eq!(limit, crate::DEVICE_CAPACITY_BYTES as u64);
}

#[test]
fn checkpoint_restores_threads_and_regions() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = hk_syscall::SYS_THREAD_CREATE;
    context.rdi = 0x4000;
    context.rsi = 0x8000;
    context.rdx = 0;
    context.r10 = 0x7000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let thread = context.rax;
    assert_ne!(thread, u64::MAX);

    context.rax = hk_syscall::SYS_CAPABILITY_CREATE;
    context.rdi = u64::from(hk_syscall::CapabilityKind::Anonymous.as_u8());
    context.rsi = 0x2000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;
    assert_ne!(capability, u64::MAX);

    context.rax = hk_syscall::SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = 0x2000;
    context.r10 = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let region_handle = context.rax;
    assert_ne!(region_handle, u64::MAX);
    let before = runtime
        .process(index)
        .regions
        .iter()
        .flatten()
        .find(|region| region.len == 0x2000)
        .expect("已登记 Region");
    let (virt, len, rights) = (before.virt, before.len, before.rights);
    let _ = runtime.finish(0, index);

    runtime.checkpoint("a").expect("保存检查点");
    let slot = runtime.restore_checkpoint("a").expect("恢复检查点");
    assert_eq!(slot, index);

    let restored: Vec<_> = runtime
        .tasks()
        .into_iter()
        .filter(|task| task.state != "exited" && task.name.starts_with('a'))
        .collect();
    assert_eq!(restored.len(), 2, "主线程与附加线程都应恢复");

    let region = runtime
        .process(slot)
        .regions
        .iter()
        .flatten()
        .find(|region| region.virt == virt)
        .expect("恢复 Region");
    assert_eq!(region.len, len);
    assert_eq!(region.rights, rights);
}

#[test]
fn enumerates_owned_objects() {
    let platform = MockPlatform::new();
    let written = Arc::clone(&platform.written);
    let mut runtime = runtime_on("b", platform);
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = hk_syscall::SYS_CAPABILITY_CREATE;
    context.rdi = u64::from(hk_syscall::CapabilityKind::Anonymous.as_u8());
    context.rsi = 0x2000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_ne!(context.rax, u64::MAX);

    context.rax = hk_syscall::SYS_OBJECT_ENUM;
    context.rdi = 0;
    context.rsi = 0;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let count = context.rax;
    assert!(count >= 2, "至少包含 Capacity 与能力记录");

    context.rax = hk_syscall::SYS_OBJECT_ENUM;
    context.rdi = 0;
    context.rsi = 0x1000;
    context.rdx = 64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 64);
    let record = written.lock();
    assert_eq!(record.len(), 64);
    assert_eq!(record[0], 0, "第一条记录是 Capacity");
    let _ = runtime.finish(0, index);
}

#[test]
fn checkpoint_records_object_graph() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    context.rax = hk_syscall::SYS_CAPABILITY_CREATE_LAZY;
    context.rdi = u64::from(hk_syscall::CapabilityKind::Anonymous.as_u8());
    context.rsi = 0x1000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_ne!(context.rax, u64::MAX);
    let _ = runtime.finish(0, index);

    let size = runtime.checkpoint("a").expect("保存检查点");
    let image = runtime.checkpoint_chunk("a", 0, size).expect("读取快照");
    let snapshot = hk_checkpoint::Snapshot::decode(&image).expect("解码快照");
    assert!(!snapshot.objects.is_empty(), "快照应记录对象图");
    assert!(snapshot.objects.iter().any(|object| object.kind == 2));
    runtime.restore_checkpoint("a").expect("恢复检查点");
}

#[test]
fn reports_self_cadence_and_meter() {
    let mut runtime = runtime();
    let Step::Resume { mut context, .. } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = hk_syscall::SYS_CADENCE_INFO;
    context.rdi = 0;
    context.rsi = 0x1000;
    context.rdx = hk_cadence::CadenceSnapshot::ENCODED_LEN as u64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, hk_cadence::CadenceSnapshot::ENCODED_LEN as u64);

    context.rax = hk_syscall::SYS_METER_INFO;
    context.rdx = hk_cadence::MeterSnapshot::ENCODED_LEN as u64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, hk_cadence::MeterSnapshot::ENCODED_LEN as u64);
}

#[test]
fn arms_wakeup_at_sleep_deadline() {
    let mut runtime = runtime();
    let Step::Resume { mut context, .. } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    context.rax = hk_syscall::SYS_SLEEP;
    context.rdi = 3;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Block);
    // 契约时钟按 timer_ticks=7 折算为 70 000 µs，再休眠 3 个 10 ms tick。
    assert_eq!(runtime.next_wakeup(0), Some(100_000));
}

#[test]
fn memory_service_receives_user_capacity_subset() {
    let runtime = runtime_with(hk_syscall::MEMORY_SERVICE);
    assert_eq!(
        runtime.tasks[1].context.rsi, 0x5001,
        "Root Server 应把初始 Capacity 句柄显式传给服务"
    );
    let granted = runtime.tasks[1]
        .capacities
        .iter()
        .flatten()
        .next()
        .and_then(|handle| {
            runtime
                .kernel
                .capacity_ref(runtime.tasks[1].tcb, *handle)
                .ok()
        })
        .expect("Memory Server 持有容量");
    let granted = granted.lock();
    assert_eq!(granted.kind(), hk_mem::hmp::CapacityKind::User);
    assert_eq!(granted.limit(), 0x20_0000);
}

#[test]
fn memory_service_capability_allocates_from_user_capacity() {
    let mut runtime = runtime_with(hk_syscall::MEMORY_SERVICE);
    let Step::Resume { mut context, .. } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    // 让第一个任务阻塞，调度到 Memory Server。
    context.rax = SYS_RECV;
    context.rdi = 1;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Block);
    let Step::Resume {
        index: memory,
        mut context,
    } = runtime.step(0)
    else {
        panic!("Memory Server 应可运行");
    };
    assert_eq!(memory, 1);

    let user_handle = runtime.tasks[memory]
        .capacities
        .iter()
        .flatten()
        .next()
        .copied()
        .expect("User Capacity 句柄");
    let user = runtime
        .kernel
        .capacity_ref(runtime.tasks[memory].tcb, user_handle)
        .expect("User Capacity");
    let user_range = user.lock().range();

    context.rax = hk_syscall::SYS_CAPABILITY_CREATE;
    context.rdi = u64::from(hk_syscall::CapabilityKind::Anonymous.as_u8());
    context.rsi = 0x1000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;
    assert_ne!(capability, u64::MAX, "能力创建应成功");
    let (_, _, backing) = runtime
        .capability_at(memory, capability as usize)
        .expect("能力句柄");
    let backing = backing.expect("能力有常驻后备");
    assert!(
        backing.start() >= user_range.start() && backing.end() <= user_range.end(),
        "能力页必须来自 Memory Server 的 User Capacity"
    );
    let _ = runtime.finish(0, memory);
}

#[test]
fn dispatches_alloc_and_exit() {
    let mut runtime = runtime();
    assert_eq!(runtime.task_count(), 2);

    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = SYS_ALLOC;
    context.rdi = 64;
    context.rsi = 16;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0x10_0000);

    context.rax = SYS_EXIT;
    context.rdi = 3;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Exit);
    assert_eq!(runtime.finish(0, index), Some((String::from("a"), 3)));
}

#[test]
fn framebuffer_access_is_display_only() {
    let mut runtime = crate::boot::user_runtime(
        &[
            crate::ServiceSpec::new("hk-server-display"),
            crate::ServiceSpec::new("other"),
        ],
        MockPlatform::new(),
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

    let Step::Resume { index: display, .. } = runtime.step(0) else {
        panic!("显示服务应可运行");
    };
    let mut context = MockContext {
        rax: SYS_FRAMEBUFFER_INFO,
        rdi: 0x1000,
        rsi: 64,
        ..MockContext::default()
    };
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, hk_syscall::FRAMEBUFFER_DESCRIPTOR_BYTES as u64);

    context.rax = SYS_FRAMEBUFFER_MAP;
    context.rdi = 0x1000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 16);

    // 非显示服务不得访问帧缓冲。
    context.rax = SYS_YIELD;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Block);
    runtime.finish(0, display);
    let Step::Resume { index: other, .. } = runtime.step(0) else {
        panic!("普通服务应可运行");
    };
    assert_ne!(other, display);
    let mut context = MockContext {
        rax: SYS_FRAMEBUFFER_INFO,
        rdi: 0x1000,
        rsi: 64,
        ..MockContext::default()
    };
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);
    context.rax = SYS_FRAMEBUFFER_MAP;
    context.rdi = 0x1000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);
}

#[test]
fn yielded_task_is_rescheduled_after_finish() {
    let mut runtime = runtime();
    let Step::Resume {
        index: first,
        mut context,
    } = runtime.step(0)
    else {
        panic!("任务应可运行");
    };
    context.rax = SYS_YIELD;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Block);
    assert_eq!(runtime.finish(0, first), None);

    // yield 不是永久阻塞：后续调度不能返回空闲，且原任务会再次被选中。
    let mut resumed = false;
    for _ in 0..4 {
        let Step::Resume { index, mut context } = runtime.step(0) else {
            panic!("yield 后不应判定空闲");
        };
        if index == first {
            resumed = true;
        }
        context.rax = SYS_YIELD;
        assert_eq!(runtime.dispatch(0, &mut context), Switch::Block);
        runtime.finish(0, index);
    }
    assert!(resumed, "yield 的任务应被重新调度");
}

#[test]
fn reclaim_notifies_provider_before_evicting_page() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let tcb = runtime.tasks[index].tcb;
    let endpoint = runtime
        .kernel
        .create_endpoint(tcb, hk_handle::Rights::READ | hk_handle::Rights::WRITE)
        .expect("端点");

    context.rax = hk_syscall::SYS_CAPABILITY_CREATE_LAZY;
    context.rdi = u64::from(hk_syscall::CapabilityKind::Anonymous.as_u8());
    context.rsi = 0x1000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;
    assert_ne!(capability, u64::MAX);
    context.rax = hk_syscall::SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = 0x1000;
    context.r10 = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let first = context.rax;
    assert_ne!(first, u64::MAX);
    context.rax = hk_syscall::SYS_REGION_MAP;
    context.rdi = capability;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let region = context.rax;
    assert_ne!(region, u64::MAX);
    let (_, _, virt, _) = runtime.region_at(index, region as usize).expect("Region");
    assert!(runtime.anonymous_fault(0, virt));
    let (handle, _, _) = runtime
        .capability_at(index, capability as usize)
        .expect("能力");
    runtime
        .kernel
        .bind_capability_provider(tcb, handle, endpoint, hk_mem::hmp::ProviderKind::Anonymous)
        .expect("绑定 Provider");
    let id = runtime.kernel.handle_id(tcb, handle).expect("能力标识");
    assert_eq!(runtime.reclaim_zero_pages(0, 0x1000), 0x1000);

    let received = runtime
        .kernel
        .recv_from(tcb, endpoint, None)
        .expect("接收")
        .expect("驱逐通知");
    assert_eq!(received.message.label, hk_syscall::PROVIDER_EVICT);
    assert_eq!(received.message.words[0], id.get());
    assert_eq!(received.message.words[2], hk_hal::PAGE_SIZE as u64);
    let _ = runtime.finish(0, index);
}

#[test]
fn numa_migrate_moves_resident_page() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let tcb = runtime.tasks[index].tcb;

    context.rax = hk_syscall::SYS_CAPABILITY_CREATE_LAZY;
    context.rdi = u64::from(hk_syscall::CapabilityKind::Anonymous.as_u8());
    context.rsi = 0x1000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;
    assert_ne!(capability, u64::MAX);
    context.rax = hk_syscall::SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = 0x1000;
    context.r10 = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let region = context.rax;
    assert_ne!(region, u64::MAX);
    let (_, _, virt, _) = runtime.region_at(index, region as usize).expect("Region");
    assert!(runtime.anonymous_fault(0, virt));

    let (handle, _, _) = runtime
        .capability_at(index, capability as usize)
        .expect("能力");
    let old = runtime
        .kernel
        .capability_page(tcb, handle, 0)
        .expect("查询")
        .expect("已装入页");
    let mut page = [0_u8; hk_hal::PAGE_SIZE];
    page[..16].copy_from_slice(b"numa migration!!");
    assert!(runtime.platform.write_phys(old.start(), &page));
    runtime.platform.node = 0;

    context.rax = hk_syscall::SYS_NUMA_MIGRATE;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = hk_hal::PAGE_SIZE as u64;
    context.r10 = 1;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, hk_hal::PAGE_SIZE as u64);
    let new = runtime
        .kernel
        .capability_page(tcb, handle, 0)
        .expect("查询")
        .expect("迁移后的页");
    assert_ne!(new.start(), old.start());
    let mut readback = [0_u8; hk_hal::PAGE_SIZE];
    assert!(runtime.platform.read_phys(new.start(), &mut readback));
    assert_eq!(readback, page);
    assert!(
        runtime
            .oom
            .events()
            .any(|event| event.kind == crate::runtime::oom::MEMORY_EVENT_MIGRATE)
    );
    let _ = runtime.finish(0, index);
}

#[test]
fn provider_death_faults_pending_caller() {
    let mut runtime = runtime_with(hk_syscall::MEMORY_SERVICE);
    let Step::Resume {
        index: _,
        mut context,
    } = runtime.step(0)
    else {
        panic!("任务应可运行");
    };

    context.rax = SYS_CAPABILITY_CREATE_LAZY;
    context.rdi = u64::from(hk_syscall::CapabilityKind::Surface.as_u8());
    context.rsi = 0x2000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;

    context.rax = SYS_CAPABILITY_BIND_PROVIDER;
    context.rdi = capability;
    context.rsi = 2;
    context.rdx = u64::from(hk_syscall::PROVIDER_KIND_SURFACE);
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);

    context.rax = SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = 0x2000;
    context.r10 = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);

    assert!(runtime.provider_fault(
        0,
        hk_base::user::AUTO_WINDOW_BASE,
        hk_mem::hmp::provider::ProviderAccess::Read,
        MockContext::default(),
    ));
    assert_eq!(runtime.tasks()[0].pending, "provider-fault");

    assert!(runtime.terminate(hk_syscall::MEMORY_SERVICE, 0));
    // Provider 退出时端点服务器失效，等待中的缺页任务以故障码退出。
    match runtime.step(0) {
        Step::Done => {}
        Step::Blocked => {}
        Step::Resume { index, .. } => panic!("任务 {index} 仍在运行"),
    }
    assert_eq!(runtime.tasks()[0].exit_code, Some(0x5052_4f56));
}

#[test]
fn provider_death_releases_reclaim_waiter() {
    let mut runtime = runtime_with(hk_syscall::MEMORY_SERVICE);
    let Step::Resume {
        index: _,
        mut context,
    } = runtime.step(0)
    else {
        panic!("任务应可运行");
    };

    context.rax = SYS_CAPABILITY_CREATE_LAZY;
    context.rdi = u64::from(hk_syscall::CapabilityKind::Anonymous.as_u8());
    context.rsi = 0x1000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;
    assert_ne!(capability, u64::MAX);

    context.rax = SYS_CAPABILITY_BIND_PROVIDER;
    context.rdi = capability;
    context.rsi = 2;
    context.rdx = u64::from(hk_syscall::PROVIDER_KIND_ANONYMOUS);
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);

    context.rax = SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = 0x1000;
    context.r10 = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);

    // 先让 Provider 提供一页并写入非零内容。
    assert!(runtime.provider_fault(
        0,
        hk_base::user::AUTO_WINDOW_BASE,
        hk_mem::hmp::provider::ProviderAccess::Read,
        MockContext::default(),
    ));
    let Step::Resume {
        index: provider,
        mut context,
    } = runtime.step(0)
    else {
        panic!("Provider 应可运行");
    };
    assert_eq!(provider, 1);
    context.rax = SYS_RECV;
    context.rdi = 2;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    context.rax = hk_syscall::SYS_CAPACITY_ALLOC;
    context.rdi = 0x5001;
    context.rsi = 4096;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let frame = context.rax;
    context.rax = hk_syscall::SYS_REPLY_HANDLE;
    context.rdi = hk_syscall::PROVIDER_PROVIDE;
    context.rsi = 0;
    context.rdx = frame;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);

    let Step::Resume {
        index: _,
        mut context,
    } = runtime.step(0)
    else {
        panic!("缺页任务应恢复");
    };

    let tcb = runtime.tasks[0].tcb;
    let (handle, _, _) = runtime.capability_at(0, capability as usize).expect("能力");
    let page = runtime
        .kernel
        .capability_page(tcb, handle, 0)
        .expect("查询")
        .expect("已装入页");
    let data = [0_u8; hk_hal::PAGE_SIZE];
    assert!(runtime.platform.write_phys(page.start(), &data));

    let provider_endpoint = runtime
        .kernel
        .capability_provider(tcb, handle)
        .expect("查询 Provider")
        .expect("Provider 端点");
    assert!(
        runtime.kernel.endpoint_served(provider_endpoint),
        "Provider 端点仍在服务"
    );
    context.rax = SYS_CAPABILITY_SWAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = hk_hal::PAGE_SIZE as u64;
    assert_eq!(
        runtime.dispatch(0, &mut context),
        Switch::Block,
        "swap result {}",
        context.rax
    );
    assert_eq!(runtime.tasks()[0].pending, "provider-reclaim");

    assert!(runtime.terminate(hk_syscall::MEMORY_SERVICE, 0));
    let Step::Resume { index, context } = runtime.step(0) else {
        panic!("换出调用方应恢复");
    };
    assert_eq!(index, 0);
    assert_eq!(context.rax, u64::MAX);
    assert_eq!(runtime.tasks()[0].pending, "-");
    assert!(
        runtime
            .kernel
            .capability_page(tcb, handle, 0)
            .expect("查询")
            .is_some(),
        "Provider 失败时页保持常驻"
    );
}

#[test]
fn capability_writeback_notifies_provider() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let tcb = runtime.tasks[index].tcb;
    let endpoint = runtime
        .kernel
        .create_endpoint(tcb, hk_handle::Rights::READ | hk_handle::Rights::WRITE)
        .expect("端点");
    let capability = hk_mem::hmp::Capability::new(
        hk_mem::hmp::CapabilityKind::Revision,
        hk_hal::PAGE_SIZE as u64,
        hk_hal::PAGE_SIZE,
    )
    .expect("能力");
    let handle = runtime
        .kernel
        .create_capability(
            tcb,
            capability,
            None,
            hk_handle::Rights::READ | hk_handle::Rights::WRITE | hk_handle::Rights::ADMIN,
        )
        .expect("能力句柄");
    let id = runtime.kernel.handle_id(tcb, handle).expect("能力标识");
    runtime
        .kernel
        .bind_capability_provider(tcb, handle, endpoint, hk_mem::hmp::ProviderKind::Revision)
        .expect("绑定 Provider");
    let number = runtime.install_capability(index, handle, None);

    context.rax = hk_syscall::SYS_CAPABILITY_WRITEBACK;
    context.rdi = number;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);
    let received = runtime
        .kernel
        .recv_from(tcb, endpoint, None)
        .expect("接收")
        .expect("回写通知");
    assert_eq!(received.message.label, hk_syscall::PROVIDER_WRITEBACK);
    assert_eq!(received.message.words[0], id.get());
}

#[test]
fn handle_id_is_queryable() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let tcb = runtime.tasks[index].tcb;
    let capacity = hk_mem::hmp::Capacity::new(
        hk_mem::hmp::CapacityId::new(97),
        hk_hal::PhysRange::new(hk_hal::PhysAddr::new(0x61_0000), 0x1_0000),
        hk_hal::PAGE_SIZE,
        hk_mem::hmp::CapacityKind::User,
        0,
    )
    .expect("Capacity");
    let handle = runtime
        .kernel
        .create_capacity(
            tcb,
            capacity,
            hk_handle::Rights::READ | hk_handle::Rights::WRITE,
        )
        .expect("创建 Capacity");
    let slot = runtime.tasks[index].capacities.len();
    runtime.tasks[index].capacities.push(Some(handle));
    let expected = runtime.kernel.handle_id(tcb, handle).expect("对象标识");

    context.rax = hk_syscall::SYS_HANDLE_ID;
    context.rdi = 0x5001 + slot as u64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, expected.get());
}

#[test]
fn capacity_frame_lifecycle() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let tcb = runtime.tasks[index].tcb;
    let capacity = hk_mem::hmp::Capacity::new(
        hk_mem::hmp::CapacityId::new(99),
        hk_hal::PhysRange::new(hk_hal::PhysAddr::new(0x50_0000), 0x1_0000),
        hk_hal::PAGE_SIZE,
        hk_mem::hmp::CapacityKind::User,
        0,
    )
    .expect("Capacity");
    let handle = runtime
        .kernel
        .create_capacity(
            tcb,
            capacity,
            hk_handle::Rights::READ
                | hk_handle::Rights::WRITE
                | hk_handle::Rights::TRANSFER
                | hk_handle::Rights::ADMIN,
        )
        .expect("创建 Capacity");
    runtime.tasks[index].capacities.push(Some(handle));
    let capacity_number = 0x5001;
    let before = runtime.process(index).ledger.charged();

    context.rax = hk_syscall::SYS_CAPACITY_ALLOC;
    context.rdi = capacity_number;
    context.rsi = hk_hal::PAGE_SIZE as u64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let frame = context.rax;
    assert_ne!(frame, u64::MAX);
    assert_eq!(
        runtime.process(index).ledger.charged(),
        before + hk_hal::PAGE_SIZE as u64,
        "Frame 分配必须计入任务 Ledger"
    );

    context.rax = hk_syscall::SYS_FRAME_PIN;
    context.rdi = frame;
    context.rsi = 1;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);
    context.rax = hk_syscall::SYS_FRAME_PIN;
    context.rsi = 0;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);

    context.rax = hk_syscall::SYS_CAPACITY_FREE;
    context.rdi = capacity_number;
    context.rsi = frame;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 1);
    assert!(runtime.tasks[index].frames.iter().all(Option::is_none));
    assert_eq!(
        runtime.process(index).ledger.charged(),
        before,
        "归还 Frame 必须释放任务 Ledger"
    );
    let _ = runtime.finish(0, index);
}

#[test]
fn duplicate_frame_defers_capacity_free_until_last_handle() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let tcb = runtime.tasks[index].tcb;
    let capacity = hk_mem::hmp::Capacity::new(
        hk_mem::hmp::CapacityId::new(102),
        hk_hal::PhysRange::new(hk_hal::PhysAddr::new(0x80_0000), 0x2000),
        hk_hal::PAGE_SIZE,
        hk_mem::hmp::CapacityKind::User,
        0,
    )
    .expect("Capacity");
    let handle = runtime
        .kernel
        .create_capacity(
            tcb,
            capacity,
            hk_handle::Rights::READ
                | hk_handle::Rights::WRITE
                | hk_handle::Rights::TRANSFER
                | hk_handle::Rights::ADMIN,
        )
        .expect("创建 Capacity");
    runtime.tasks[index].capacities.push(Some(handle));

    context.rax = hk_syscall::SYS_CAPACITY_ALLOC;
    context.rdi = 0x5001;
    context.rsi = hk_hal::PAGE_SIZE as u64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let frame = context.rax;
    assert_ne!(frame, u64::MAX);
    let charged = runtime.process(index).ledger.charged();

    context.rax = hk_syscall::SYS_HANDLE_DUPLICATE;
    context.rdi = frame;
    context.rsi = u64::from(
        hk_syscall::RIGHT_READ
            | hk_syscall::RIGHT_WRITE
            | hk_syscall::RIGHT_TRANSFER
            | hk_syscall::RIGHT_ADMIN,
    );
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let duplicate = context.rax;
    assert_ne!(duplicate, u64::MAX);
    assert_eq!(
        runtime.process(index).ledger.charged(),
        charged,
        "复制 Frame 句柄不能重复计费"
    );

    context.rax = hk_syscall::SYS_CAPACITY_FREE;
    context.rdi = 0x5001;
    context.rsi = frame;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX, "仍有其它句柄时不能归还物理页");

    context.rax = hk_syscall::SYS_HANDLE_CLOSE;
    context.rdi = duplicate;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);

    context.rax = hk_syscall::SYS_CAPACITY_FREE;
    context.rdi = 0x5001;
    context.rsi = frame;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 1);
    assert_eq!(runtime.process(index).ledger.charged(), 0);
    let _ = runtime.finish(0, index);
}

#[test]
fn duplicate_capability_does_not_double_charge() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let before = runtime.process(index).ledger.charged();

    context.rax = hk_syscall::SYS_CAPABILITY_CREATE;
    context.rdi = u64::from(hk_syscall::CapabilityKind::Anonymous.as_u8());
    context.rsi = 0x2000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;
    assert_ne!(capability, u64::MAX);
    assert_eq!(
        runtime.process(index).ledger.charged(),
        before + 0x2000,
        "能力驻留内存计入任务 Ledger"
    );

    context.rax = hk_syscall::SYS_HANDLE_DUPLICATE;
    context.rdi = capability;
    context.rsi = u64::from(
        hk_syscall::RIGHT_READ
            | hk_syscall::RIGHT_WRITE
            | hk_syscall::RIGHT_TRANSFER
            | hk_syscall::RIGHT_ADMIN,
    );
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let duplicate = context.rax;
    assert_ne!(duplicate, u64::MAX);
    assert_eq!(
        runtime.process(index).ledger.charged(),
        before + 0x2000,
        "复制能力句柄不能重复计费"
    );

    context.rax = hk_syscall::SYS_HANDLE_CLOSE;
    context.rdi = duplicate;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    context.rax = hk_syscall::SYS_HANDLE_CLOSE;
    context.rdi = capability;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(runtime.process(index).ledger.charged(), before);
    let _ = runtime.finish(0, index);
}

#[test]
fn capacity_windows_reports_all_windows() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let capacity = 0x5001;

    context.rax = hk_syscall::SYS_CAPACITY_WINDOWS;
    context.rdi = capacity;
    context.rsi = 0;
    context.rdx = 0;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 1);

    context.rax = hk_syscall::SYS_CAPACITY_WINDOWS;
    context.rdi = capacity;
    context.rsi = 0x2000;
    context.rdx = hk_syscall::CAPACITY_WINDOW_BYTES as u64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, hk_syscall::CAPACITY_WINDOW_BYTES as u64);

    let data = runtime.platform.written.lock();
    let start = u64::from_le_bytes(data[..8].try_into().expect("start"));
    let len = u64::from_le_bytes(data[8..16].try_into().expect("len"));
    assert_eq!(start, 0x9000_0000);
    assert_eq!(len, 0x20_0000);
    drop(data);
    let _ = runtime.finish(0, index);
}

#[test]
fn closing_capacity_handle_revokes_allocation() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let before = runtime.process(index).ledger.charged();

    context.rax = hk_syscall::SYS_HANDLE_CLOSE;
    context.rdi = 0x5001;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);

    context.rax = hk_syscall::SYS_CAPABILITY_CREATE;
    context.rdi = u64::from(hk_syscall::CapabilityKind::Anonymous.as_u8());
    context.rsi = hk_hal::PAGE_SIZE as u64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX, "关闭 Capacity 后不能再分配");
    assert_eq!(runtime.process(index).ledger.charged(), before);
    let _ = runtime.finish(0, index);
}

#[test]
fn restart_reuses_exited_slot() {
    let mut runtime = runtime();
    let slot = runtime
        .tasks
        .iter()
        .position(|task| task.name == "b")
        .expect("服务 b");
    let count = runtime.task_count();
    for _ in 0..4 {
        assert!(runtime.terminate("b", 0));
        assert!(runtime.tasks[slot].exited);
        assert_eq!(runtime.reuse_slot("b"), slot);
        assert_eq!(runtime.process(slot).ledger.charged(), 0);

        let endpoint = runtime
            .start_at(
                slot,
                "b",
                false,
                hk_cadence::DEFAULT_SPEC,
                MockContext::default(),
                None,
            )
            .expect("重启服务");
        assert_eq!(endpoint, slot as u64 + 1);
        assert_eq!(runtime.task_count(), count, "重启必须复用槽位");
        assert_eq!(runtime.tasks[slot].name, "b");
        assert!(!runtime.tasks[slot].exited);
        assert_eq!(runtime.process(slot).ledger.charged(), 0);
        assert!(runtime.tasks[slot].capacities[0].is_some());
    }
}

#[test]
fn exit_releases_frame_charges() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let tcb = runtime.tasks[index].tcb;
    let capacity = hk_mem::hmp::Capacity::new(
        hk_mem::hmp::CapacityId::new(103),
        hk_hal::PhysRange::new(hk_hal::PhysAddr::new(0x82_0000), 0x2000),
        hk_hal::PAGE_SIZE,
        hk_mem::hmp::CapacityKind::User,
        0,
    )
    .expect("Capacity");
    let handle = runtime
        .kernel
        .create_capacity(
            tcb,
            capacity,
            hk_handle::Rights::READ
                | hk_handle::Rights::WRITE
                | hk_handle::Rights::TRANSFER
                | hk_handle::Rights::ADMIN,
        )
        .expect("创建 Capacity");
    runtime.tasks[index].capacities.push(Some(handle));

    context.rax = hk_syscall::SYS_CAPACITY_ALLOC;
    context.rdi = 0x5001;
    context.rsi = hk_hal::PAGE_SIZE as u64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_ne!(context.rax, u64::MAX);
    assert!(runtime.process(index).ledger.charged() > 0);

    context.rax = hk_syscall::SYS_EXIT;
    context.rdi = 0;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Exit);
    assert_eq!(
        runtime.process(index).ledger.charged(),
        0,
        "任务退出必须释放仍持有的 Frame 记账"
    );
    assert!(runtime.tasks[index].frames.iter().all(Option::is_none));
    let _ = runtime.finish(0, index);
}

#[test]
fn receiving_frame_charges_receiver_ledger() {
    let mut runtime = runtime();
    let Step::Resume { index, .. } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let tcb = runtime.tasks[index].tcb;
    let capacity = hk_mem::hmp::Capacity::new(
        hk_mem::hmp::CapacityId::new(101),
        hk_hal::PhysRange::new(hk_hal::PhysAddr::new(0x70_0000), 0x2000),
        hk_hal::PAGE_SIZE,
        hk_mem::hmp::CapacityKind::User,
        0,
    )
    .expect("Capacity");
    let capacity = runtime
        .kernel
        .create_capacity(
            tcb,
            capacity,
            hk_handle::Rights::READ
                | hk_handle::Rights::WRITE
                | hk_handle::Rights::TRANSFER
                | hk_handle::Rights::ADMIN,
        )
        .expect("创建 Capacity");
    let frame = runtime
        .kernel
        .allocate_frame(tcb, capacity, hk_hal::PAGE_SIZE, None)
        .expect("Frame");

    let before = runtime.process(index).ledger.charged();
    let installed = runtime
        .install_extra_capability(index, frame)
        .expect("安装 Frame");
    assert_eq!(
        runtime.process(index).ledger.charged(),
        before + hk_hal::PAGE_SIZE as u64,
        "接收 Frame 必须按物理区间记账"
    );
    assert_eq!(runtime.close_handle(index, tcb, installed as usize), 0);
    assert_eq!(
        runtime.process(index).ledger.charged(),
        before,
        "关闭接收的 Frame 归还记账"
    );
    let _ = runtime.finish(0, index);
}

#[test]
fn frame_maps_and_returns_to_capacity() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let tcb = runtime.tasks[index].tcb;
    let capacity = hk_mem::hmp::Capacity::new(
        hk_mem::hmp::CapacityId::new(100),
        hk_hal::PhysRange::new(hk_hal::PhysAddr::new(0x60_0000), 0x2000),
        hk_hal::PAGE_SIZE,
        hk_mem::hmp::CapacityKind::User,
        0,
    )
    .expect("Capacity");
    let handle = runtime
        .kernel
        .create_capacity(
            tcb,
            capacity,
            hk_handle::Rights::READ
                | hk_handle::Rights::WRITE
                | hk_handle::Rights::TRANSFER
                | hk_handle::Rights::ADMIN,
        )
        .expect("创建 Capacity");
    let capacity_handle = handle;
    runtime.tasks[index].capacities.push(Some(handle));
    let capacity_number = 0x5000 + runtime.tasks[index].capacities.len() as u64;
    let before = runtime.process(index).ledger.charged();

    context.rax = hk_syscall::SYS_CAPACITY_ALLOC;
    context.rdi = capacity_number;
    context.rsi = hk_hal::PAGE_SIZE as u64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let frame = context.rax;
    assert_eq!(
        runtime.process(index).ledger.charged(),
        before + hk_hal::PAGE_SIZE as u64
    );
    assert_eq!(
        runtime
            .kernel
            .capacity_info(tcb, capacity_handle)
            .expect("Capacity")
            .4,
        hk_hal::PAGE_SIZE as u64
    );

    context.rax = hk_syscall::SYS_FRAME_MAP;
    context.rdi = frame;
    context.rsi = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let region = context.rax;
    assert_ne!(region, u64::MAX);
    assert!(runtime.tasks[index].frames.iter().all(Option::is_none));
    assert_eq!(
        runtime.process(index).ledger.charged(),
        before + hk_hal::PAGE_SIZE as u64,
        "映射只转移 Frame 记账，不能重复计费"
    );
    assert_eq!(runtime.process(index).regions.iter().flatten().count(), 1);
    assert!(
        runtime
            .process(index)
            .regions
            .iter()
            .flatten()
            .all(|region| region.internal_capability)
    );
    assert_eq!(
        runtime
            .kernel
            .capacity_info(tcb, capacity_handle)
            .expect("Capacity")
            .4,
        hk_hal::PAGE_SIZE as u64,
        "映射后物理页仍由 Capacity 记账"
    );

    context.rax = hk_syscall::SYS_REGION_UNMAP;
    context.rdi = region;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 1);
    assert!(runtime.process(index).regions.iter().all(Option::is_none));
    assert_eq!(
        runtime.process(index).ledger.charged(),
        before,
        "解除映射必须归还映射记账"
    );
    assert_eq!(
        runtime
            .kernel
            .capacity_info(tcb, capacity_handle)
            .expect("Capacity")
            .4,
        0,
        "解除映射必须把物理页归还 Capacity"
    );
    let _ = runtime.finish(0, index);
}

#[test]
fn pinned_frame_maps_and_returns_to_capacity() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let tcb = runtime.tasks[index].tcb;
    let capacity = hk_mem::hmp::Capacity::new(
        hk_mem::hmp::CapacityId::new(101),
        hk_hal::PhysRange::new(hk_hal::PhysAddr::new(0x80_0000), 0x2000),
        hk_hal::PAGE_SIZE,
        hk_mem::hmp::CapacityKind::User,
        0,
    )
    .expect("Capacity");
    let capacity_handle = runtime
        .kernel
        .create_capacity(
            tcb,
            capacity,
            hk_handle::Rights::READ
                | hk_handle::Rights::WRITE
                | hk_handle::Rights::TRANSFER
                | hk_handle::Rights::ADMIN,
        )
        .expect("创建 Capacity");
    runtime.tasks[index].capacities.push(Some(capacity_handle));
    let capacity_number = 0x5000 + runtime.tasks[index].capacities.len() as u64;
    let before = runtime.process(index).ledger.charged();

    context.rax = hk_syscall::SYS_CAPACITY_ALLOC;
    context.rdi = capacity_number;
    context.rsi = hk_hal::PAGE_SIZE as u64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let frame = context.rax;

    // 先固定再映射：pin 不随 OwnedFrame 转移，解映射时不得因 pinned 拒绝归还。
    context.rax = hk_syscall::SYS_FRAME_PIN;
    context.rdi = frame;
    context.rsi = 1;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0, "pin 应成功");

    context.rax = hk_syscall::SYS_FRAME_MAP;
    context.rdi = frame;
    context.rsi = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let region = context.rax;
    assert_ne!(region, u64::MAX, "pinned Frame 仍应可映射");

    context.rax = hk_syscall::SYS_REGION_UNMAP;
    context.rdi = region;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 1, "解映射应成功");
    assert_eq!(
        runtime.process(index).ledger.charged(),
        before,
        "解映射必须归还映射记账"
    );
    assert_eq!(
        runtime
            .kernel
            .capacity_info(tcb, capacity_handle)
            .expect("Capacity")
            .4,
        0,
        "pinned Frame 解映射后仍须归还物理页"
    );
    let _ = runtime.finish(0, index);
}

#[test]
fn shared_user_capacity_grants_no_admin() {
    let runtime = runtime_with("b");
    let tcb = runtime.tasks[1].tcb;
    let handle = runtime.tasks[1].capacities[0].expect("服务应有初始容量句柄");
    let rights = runtime
        .kernel
        .handle_rights(tcb, handle, hk_handle::ObjectKind::Capacity)
        .expect("容量权限");
    assert!(
        rights.contains(hk_handle::Rights::READ | hk_handle::Rights::WRITE),
        "服务需要读写以分配与查询容量"
    );
    assert!(
        !rights.contains(hk_handle::Rights::ADMIN),
        "共享 User Capacity 不得授予拆分/合并权限"
    );
    assert!(
        !rights.contains(hk_handle::Rights::TRANSFER),
        "共享 User Capacity 不随服务转移"
    );
    assert!(
        runtime
            .kernel
            .split_capacity(
                tcb,
                handle,
                hk_hal::PAGE_SIZE,
                hk_mem::hmp::CapacityKind::User,
                0,
                None
            )
            .is_err(),
        "缺少 ADMIN 的服务不能拆分共享 User Capacity"
    );
}

#[test]
fn rejected_frame_map_keeps_frame_handle() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let tcb = runtime.tasks[index].tcb;
    let capacity = hk_mem::hmp::Capacity::new(
        hk_mem::hmp::CapacityId::new(102),
        hk_hal::PhysRange::new(hk_hal::PhysAddr::new(0x82_0000), 0x2000),
        hk_hal::PAGE_SIZE,
        hk_mem::hmp::CapacityKind::User,
        0,
    )
    .expect("Capacity");
    let capacity_handle = runtime
        .kernel
        .create_capacity(
            tcb,
            capacity,
            hk_handle::Rights::READ
                | hk_handle::Rights::WRITE
                | hk_handle::Rights::TRANSFER
                | hk_handle::Rights::ADMIN,
        )
        .expect("创建 Capacity");
    runtime.tasks[index].capacities.push(Some(capacity_handle));
    let capacity_number = 0x5000 + runtime.tasks[index].capacities.len() as u64;

    context.rax = hk_syscall::SYS_CAPACITY_ALLOC;
    context.rdi = capacity_number;
    context.rsi = hk_hal::PAGE_SIZE as u64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let frame = context.rax;

    // W^X 冲突：Region 校验失败，调用方 Frame 必须原样保留。
    context.rax = hk_syscall::SYS_FRAME_MAP;
    context.rdi = frame;
    context.rsi = 0b110;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX);
    assert!(
        runtime.handle_at(index, frame as usize).is_some(),
        "映射失败不得销毁 Frame 句柄"
    );
    assert_eq!(
        runtime
            .kernel
            .capacity_info(tcb, capacity_handle)
            .expect("Capacity")
            .4,
        hk_hal::PAGE_SIZE as u64,
        "失败不得归还或清零物理页"
    );

    // 合法参数下仍可映射，证明句柄未被破坏。
    context.rax = hk_syscall::SYS_FRAME_MAP;
    context.rdi = frame;
    context.rsi = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let mapped = context.rax;
    assert_ne!(mapped, u64::MAX, "合法映射失败");
    let _ = runtime.finish(0, index);
}

#[test]
fn protect_to_guard_unmaps_region() {
    let platform = MockPlatform::new();
    let unmapped = Arc::clone(&platform.unmapped);
    let mut runtime = runtime_on("b", platform);
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = hk_syscall::SYS_CAPABILITY_CREATE;
    context.rdi = 4;
    context.rsi = 0x2000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;

    context.rax = hk_syscall::SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = 0x1000;
    context.r10 = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let region = context.rax;
    assert_ne!(region, u64::MAX);
    let (_, _, virt, _) = runtime.region_at(index, region as usize).expect("Region");

    let before = unmapped.load(Ordering::Acquire);
    context.rax = hk_syscall::SYS_REGION_PROTECT;
    context.rdi = region;
    context.rsi = 0b11 | ((hk_mem::hmp::RegionFlags::GUARD.bits() as u64) << 32);
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0, "protect 为 GUARD 应成功");
    assert!(
        unmapped.load(Ordering::Acquire) > before,
        "GUARD 区域必须拆除页表映射"
    );

    assert_eq!(
        runtime.handle_fault(
            0,
            virt,
            hk_mem::hmp::provider::ProviderAccess::Read,
            MockContext::default()
        ),
        FaultOutcome::Unhandled,
        "GUARD 命中必须落到故障路径"
    );
    let _ = runtime.finish(0, index);
}

#[test]
fn frame_map_honours_pinned_flag() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let tcb = runtime.tasks[index].tcb;
    let capacity = hk_mem::hmp::Capacity::new(
        hk_mem::hmp::CapacityId::new(106),
        hk_hal::PhysRange::new(hk_hal::PhysAddr::new(0x8c_0000), 0x2000),
        hk_hal::PAGE_SIZE,
        hk_mem::hmp::CapacityKind::User,
        0,
    )
    .expect("Capacity");
    let capacity_handle = runtime
        .kernel
        .create_capacity(
            tcb,
            capacity,
            hk_handle::Rights::READ
                | hk_handle::Rights::WRITE
                | hk_handle::Rights::TRANSFER
                | hk_handle::Rights::ADMIN,
        )
        .expect("创建 Capacity");
    runtime.tasks[index].capacities.push(Some(capacity_handle));
    let capacity_number = 0x5000 + runtime.tasks[index].capacities.len() as u64;

    context.rax = hk_syscall::SYS_CAPACITY_ALLOC;
    context.rdi = capacity_number;
    context.rsi = hk_hal::PAGE_SIZE as u64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let frame = context.rax;

    context.rax = hk_syscall::SYS_FRAME_MAP;
    context.rdi = frame;
    context.rsi = 0b11 | ((hk_mem::hmp::RegionFlags::PINNED.bits() as u64) << 32);
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let region = context.rax;
    assert_ne!(region, u64::MAX);

    let (slot, ..) = runtime.region_at(index, region as usize).expect("Region");
    let capability = runtime.process(index).regions[slot]
        .as_ref()
        .expect("映射")
        .capability;
    assert_eq!(
        runtime.kernel.capability_pinned(tcb, capability),
        Ok(true),
        "PINNED 帧映射必须固定代建能力"
    );
    let _ = runtime.finish(0, index);
}

#[test]
fn unmap_failure_keeps_region_and_retries() {
    let platform = MockPlatform::new();
    let unmap_ok = Arc::clone(&platform.unmap_ok);
    let mut runtime = runtime_on("b", platform);
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = hk_syscall::SYS_CAPABILITY_CREATE_LAZY;
    context.rdi = u64::from(hk_syscall::CapabilityKind::Anonymous.as_u8());
    context.rsi = 0x2000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;
    assert_ne!(capability, u64::MAX);

    context.rax = hk_syscall::SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = 0x2000;
    context.r10 = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let region = context.rax;
    assert_ne!(region, u64::MAX);
    let (slot, ..) = runtime.region_at(index, region as usize).expect("Region");
    let charged = runtime.process(index).ledger.charged();

    unmap_ok.store(false, Ordering::Release);
    context.rax = hk_syscall::SYS_REGION_UNMAP;
    context.rdi = region;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0, "解除失败必须报错");
    assert!(
        runtime.process(index).regions[slot].is_some(),
        "失败时必须保留 Region 元数据"
    );
    assert_eq!(
        runtime.process(index).ledger.charged(),
        charged,
        "失败时不得释放记账"
    );

    unmap_ok.store(true, Ordering::Release);
    context.rax = hk_syscall::SYS_REGION_UNMAP;
    context.rdi = region;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 1, "恢复后重试应成功");
    assert!(runtime.process(index).regions[slot].is_none());
    let _ = runtime.finish(0, index);
}

#[test]
fn reclaim_releases_page_charges_of_all_aliases() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    // 同一能力映射两次，页在第二个 Region 上缺页装入并记账。
    context.rax = hk_syscall::SYS_CAPABILITY_CREATE_LAZY;
    context.rdi = u64::from(hk_syscall::CapabilityKind::Anonymous.as_u8());
    context.rsi = 0x2000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;
    assert_ne!(capability, u64::MAX);

    context.rax = hk_syscall::SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = 0x1000;
    context.r10 = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let first = context.rax;
    assert_ne!(first, u64::MAX);
    context.rax = hk_syscall::SYS_REGION_MAP;
    context.rdi = capability;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let second = context.rax;
    assert_ne!(second, u64::MAX);

    let (.., second_virt, _) = runtime.region_at(index, second as usize).expect("Region");
    assert!(runtime.anonymous_fault(0, second_virt));
    let charged = runtime.process(index).ledger.charged();
    assert_eq!(runtime.reclaim_zero_pages(0, 0x1000), 0x1000);
    assert_eq!(
        runtime.process(index).ledger.charged(),
        charged - 0x1000,
        "回收必须释放该能力在所有别名上的记账"
    );
    let _ = runtime.finish(0, index);
}

#[test]
fn region_protect_rolls_back_on_map_failure() {
    let platform = MockPlatform::new();
    let map_ok = Arc::clone(&platform.map_ok);
    let mut runtime = runtime_on("b", platform);
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = hk_syscall::SYS_CAPABILITY_CREATE;
    context.rdi = 4;
    context.rsi = 0x2000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;

    context.rax = hk_syscall::SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = 0x1000;
    context.r10 = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let region = context.rax;
    let (slot, ..) = runtime.region_at(index, region as usize).expect("Region");
    let before = runtime.process(index).regions[slot]
        .as_ref()
        .expect("映射")
        .rights;

    map_ok.store(false, Ordering::Release);
    context.rax = hk_syscall::SYS_REGION_PROTECT;
    context.rdi = region;
    context.rsi = 0b1;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX, "重映射失败必须报错");
    assert_eq!(
        runtime.process(index).regions[slot]
            .as_ref()
            .expect("映射")
            .rights,
        before,
        "失败不得提交新权限"
    );

    map_ok.store(true, Ordering::Release);
    context.rax = hk_syscall::SYS_REGION_PROTECT;
    context.rdi = region;
    context.rsi = 0b1;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0, "恢复后应可成功收窄");

    // 新增 SEALED 时映射失败：内核 Region 尚未提交，不会出现“已密封无法回滚”。
    map_ok.store(false, Ordering::Release);
    context.rax = hk_syscall::SYS_REGION_PROTECT;
    context.rdi = region;
    context.rsi = 0b1 | ((hk_mem::hmp::RegionFlags::SEALED.bits() as u64) << 32);
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX);
    assert!(
        !runtime.process(index).regions[slot]
            .as_ref()
            .expect("映射")
            .flags
            .contains(hk_mem::hmp::RegionFlags::SEALED),
        "失败不得提交 SEALED"
    );

    map_ok.store(true, Ordering::Release);
    context.rax = hk_syscall::SYS_REGION_PROTECT;
    context.rdi = region;
    context.rsi = 0b1 | ((hk_mem::hmp::RegionFlags::SEALED.bits() as u64) << 32);
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0, "重试后应成功密封");
    let _ = runtime.finish(0, index);
}

#[test]
fn frame_map_rejects_guard_with_backing() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let tcb = runtime.tasks[index].tcb;
    let capacity = hk_mem::hmp::Capacity::new(
        hk_mem::hmp::CapacityId::new(103),
        hk_hal::PhysRange::new(hk_hal::PhysAddr::new(0x84_0000), 0x2000),
        hk_hal::PAGE_SIZE,
        hk_mem::hmp::CapacityKind::User,
        0,
    )
    .expect("Capacity");
    let capacity_handle = runtime
        .kernel
        .create_capacity(
            tcb,
            capacity,
            hk_handle::Rights::READ
                | hk_handle::Rights::WRITE
                | hk_handle::Rights::TRANSFER
                | hk_handle::Rights::ADMIN,
        )
        .expect("创建 Capacity");
    runtime.tasks[index].capacities.push(Some(capacity_handle));
    let capacity_number = 0x5000 + runtime.tasks[index].capacities.len() as u64;

    context.rax = hk_syscall::SYS_CAPACITY_ALLOC;
    context.rdi = capacity_number;
    context.rsi = hk_hal::PAGE_SIZE as u64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let frame = context.rax;

    // GUARD 与常驻 backing 互斥：拒绝且不消费句柄，物理页不归还。
    context.rax = hk_syscall::SYS_FRAME_MAP;
    context.rdi = frame;
    context.rsi = 0b11 | ((hk_mem::hmp::RegionFlags::GUARD.bits() as u64) << 32);
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX);
    assert!(
        runtime.handle_at(index, frame as usize).is_some(),
        "GUARD 拒绝不得消费 Frame 句柄"
    );
    assert_eq!(
        runtime
            .kernel
            .capacity_info(tcb, capacity_handle)
            .expect("Capacity")
            .4,
        hk_hal::PAGE_SIZE as u64
    );
    let _ = runtime.finish(0, index);
}

#[test]
fn dma_map_derives_rights_and_pins() {
    let maps = Arc::new(spin::Mutex::new(Vec::new()));
    let platform = MockPlatform {
        iommu: Some(RecordingIommu {
            maps: Arc::clone(&maps),
        }),
        ..MockPlatform::new()
    };
    let mut runtime = runtime_on("b", platform);
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let tcb = runtime.tasks[index].tcb;

    let grant = hk_device::DeviceGrant {
        address: hk_device::PciAddress {
            segment: 0,
            bus: 1,
            device: 2,
            function: 0,
            offset: 0,
        },
        vendor: 0x1af4,
        device: 0x1000,
        mmio: [None; 6],
        io: [None; 6],
        dma: Some(hk_hal::PhysRange::new(
            hk_hal::PhysAddr::new(0x1_0000),
            0x2000,
        )),
        irq: None,
        capabilities: [0; 8],
        capability_count: 0,
    };
    let grant_handle = runtime
        .kernel
        .create_device_grant(
            tcb,
            grant,
            hk_handle::Rights::READ | hk_handle::Rights::WRITE | hk_handle::Rights::ADMIN,
        )
        .expect("创建设备授权");
    let grant_number = runtime.install_grant(index, grant_handle, None);

    let capacity = hk_mem::hmp::Capacity::new(
        hk_mem::hmp::CapacityId::new(105),
        hk_hal::PhysRange::new(hk_hal::PhysAddr::new(0x8a_0000), 0x2000),
        hk_hal::PAGE_SIZE,
        hk_mem::hmp::CapacityKind::User,
        0,
    )
    .expect("Capacity");
    let capacity_handle = runtime
        .kernel
        .create_capacity(
            tcb,
            capacity,
            hk_handle::Rights::READ
                | hk_handle::Rights::WRITE
                | hk_handle::Rights::TRANSFER
                | hk_handle::Rights::ADMIN,
        )
        .expect("创建 Capacity");
    runtime.tasks[index].capacities.push(Some(capacity_handle));
    let frame = runtime
        .kernel
        .allocate_frame(tcb, capacity_handle, hk_hal::PAGE_SIZE, None)
        .expect("Frame");
    let writable = runtime
        .install_extra_capability(index, frame)
        .expect("安装可写 Frame");
    let readonly_handle = runtime
        .kernel
        .duplicate_handle(tcb, frame, hk_handle::Rights::READ)
        .expect("只读副本");
    let readonly = runtime
        .install_extra_capability(index, readonly_handle)
        .expect("安装只读 Frame");

    context.rax = hk_syscall::SYS_DMA_MAP;
    context.rdi = grant_number;
    context.rsi = readonly;
    context.rdx = hk_hal::PAGE_SIZE as u64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_ne!(context.rax, u64::MAX, "只读句柄应可建立只读映射");
    assert_eq!(
        maps.lock().last().map(|(_, flags)| *flags),
        Some(hk_hal::IommuFlags::READ),
        "只读句柄只能建立只读设备映射"
    );

    context.rax = hk_syscall::SYS_DMA_MAP;
    context.rdi = grant_number;
    context.rsi = writable;
    context.rdx = hk_hal::PAGE_SIZE as u64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_ne!(context.rax, u64::MAX, "可写句柄应建立可写映射");
    assert_eq!(
        maps.lock().last().map(|(_, flags)| *flags),
        Some(hk_hal::IommuFlags::READ | hk_hal::IommuFlags::WRITE),
        "可写句柄才能建立可写设备映射"
    );
    assert!(
        runtime.kernel.frame_info(tcb, frame).expect("Frame").4,
        "DMA 映射期间必须固定 Frame"
    );

    runtime.release_dma_mappings(index, grant_handle);
    assert!(
        !runtime.kernel.frame_info(tcb, frame).expect("Frame").4,
        "解除 DMA 映射后必须解除固定"
    );
    let _ = runtime.finish(0, index);
}

#[test]
fn rejects_device_grant_without_authority() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    context.rax = hk_syscall::SYS_DEVICE_GRANT_CREATE;
    context.rdi = 0x1000;
    context.rsi = hk_device::DEVICE_GRANT_BYTES as u64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX);
    assert_eq!(runtime.tasks()[index].handles, 5 + ARENA_HANDLES);
}

#[test]
fn deferred_irq_wakes_bound_notification() {
    let mut runtime = runtime();
    let Step::Resume { index, .. } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let tcb = runtime.tasks[index].tcb;
    let notification = runtime.tasks[index].notifications[index].expect("自己的通知");
    let id = runtime
        .kernel
        .handle_id(tcb, notification)
        .expect("通知标识");
    runtime.tasks[index].bound_irqs.push((
        notification,
        11,
        hk_hal::NotificationId::new(id.get()),
        0b100,
    ));

    runtime.signal_irq(11);
    assert_eq!(
        runtime
            .kernel
            .wait(tcb, notification, 0b100)
            .expect("等待通知"),
        Some(0b100)
    );
}

#[test]
fn failed_service_wakes_inflight_recv_with_empty_response() {
    let mut runtime = runtime();
    let Step::Resume { index, .. } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let other = 1 - index;
    let handle = runtime.tasks[index].handles[index].expect("自己的端点");
    let other_tcb = runtime.tasks[other].tcb;
    runtime.tasks[index].pending = Pending::RecvData {
        handle,
        buffer: 0x1000,
        capacity: 4096,
        expected: Some(other_tcb),
        handle_out: false,
        deadline: None,
    };
    runtime.tasks[other].exited = true;

    assert!(runtime.finish(0, other).is_some());
    assert!(matches!(runtime.tasks[index].pending, Pending::None));
    assert_eq!(runtime.tasks[index].context.rax, 0);
    assert_eq!(runtime.tasks[index].context.r8, 0);
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
