//! 用户运行时单元测试。

use crate::runtime::*;
use alloc::string::String;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};
use hk_hal::{SyscallFrame, UserContext};

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
    fn resume(&self) {}

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
    unmap_ok: bool,
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
            unmap_ok: true,
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

    fn map_user(
        &self,
        _slot: usize,
        _virt: u64,
        _phys: u64,
        _len: usize,
        rights: hk_handle::Rights,
        flags: hk_mem::hmp::RegionFlags,
    ) -> bool {
        self.last_map.store(
            rights.bits() as u64 | ((flags.bits() as u64) << 32),
            Ordering::Release,
        );
        true
    }

    fn unmap_user(&self, _slot: usize, _virt: u64, _len: usize) -> bool {
        self.unmapped
            .fetch_add(1, core::sync::atomic::Ordering::AcqRel);
        self.unmap_ok
    }

    fn page_is_zero(&self, _phys: hk_hal::PhysAddr) -> Option<bool> {
        Some(self.zero)
    }

    fn page_shared(&self, _phys: hk_hal::PhysAddr) -> bool {
        self.shared
    }

    fn resolve_cow(&self, _address: u64) -> Option<CowOutcome> {
        self.cow
    }

    fn ledger_stats(&self) -> Option<Vec<hk_mem::LedgerStats>> {
        Some(self.ledgers.clone())
    }

    fn user_mapped(&self, _address: u64) -> bool {
        true
    }

    fn arena_layout(&self, slot: usize) -> (u64, u64) {
        (0x10_0000 + slot as u64 * 0x2_0000, 0x1_0000)
    }

    fn memory_limit(&self, _slot: usize) -> u64 {
        MEMORY_LIMIT
    }

    fn fork(&self, _parent: usize, _child: usize, cow_ranges: &[(u64, u64)]) -> bool {
        *self.last_cow.lock() = cow_ranges.to_vec();
        true
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

    fn framebuffer(&self) -> Option<hk_hal::FramebufferInfo> {
        Some(hk_hal::FramebufferInfo {
            phys: hk_hal::PhysAddr::new(0x8000_0000),
            width: 2,
            height: 2,
            stride: 8,
            format: hk_hal::PixelFormat::Rgb32,
        })
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
fn transfers_handle_over_ipc() {
    let mut runtime = runtime();

    let Step::Resume { index: sender, .. } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let mut context = MockContext {
        rax: SYS_YIELD,
        ..MockContext::default()
    };
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Block);

    let Step::Resume {
        index: receiver,
        mut context,
    } = runtime.step(0)
    else {
        panic!("任务应可运行");
    };
    assert_eq!(receiver, 1);
    context.rax = SYS_RECV;
    context.rdi = 2;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Block);

    let Step::Resume {
        index: sender_again,
        mut context,
    } = runtime.step(0)
    else {
        panic!("任务应可运行");
    };
    assert_eq!(sender_again, sender);
    context.rax = SYS_SEND_HANDLE;
    context.rdi = 2;
    context.rsi = 9;
    context.rdx = 5;
    context.r10 = 2;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(runtime.tasks()[receiver].state, "runnable");

    let Step::Resume { index, context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    assert_eq!(index, receiver);
    assert_eq!(context.rax, 9);
    assert_eq!(context.rdx, 5);
    assert_eq!(context.rcx, 1);
    assert_eq!(context.r8, (EXTRA_HANDLE_BASE + 1) as u64);
    assert_eq!(runtime.tasks()[receiver].handles, 6 + ARENA_HANDLES);
}

#[test]
fn arena_alloc_is_lazy_and_charged() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    assert!(
        runtime.process(index).hmp_arena.regions().is_empty(),
        "Arena 分配前不应建立 Region"
    );
    let before = runtime.process(index).ledger.charged();

    context.rax = SYS_ALLOC;
    context.rdi = 64;
    context.rsi = 16;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let start = context.rax;
    assert_eq!(start, 0x10_0000);
    assert_eq!(
        runtime.process(index).hmp_arena.regions().len(),
        1,
        "Arena 分配应登记为 Region"
    );
    assert_eq!(
        runtime.process(index).ledger.charged(),
        before + hk_hal::PAGE_SIZE as u64,
        "Arena 分配按页计入进程 Ledger"
    );
    // 首次访问才装入零页；分配本身不建立页表映射。
    assert!(runtime.anonymous_fault(0, start));
    assert_eq!(
        runtime.process(index).ledger.charged(),
        before + hk_hal::PAGE_SIZE as u64
    );
    assert_eq!(
        runtime.process_resident(index),
        hk_hal::PAGE_SIZE as u64,
        "缺页后常驻快照包含 Arena 页"
    );

    context.rax = SYS_LEDGER_INFO;
    context.rdi = 0x400;
    context.rsi = 48;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 48);
    let _ = runtime.finish(0, index);
}

#[test]
fn enumerates_capacity_ledgers() {
    let platform = MockPlatform {
        ledgers: alloc::vec![
            hk_mem::LedgerStats {
                capacity: hk_mem::hmp::CapacityId::new(7),
                kind: hk_mem::hmp::CapacityKind::User,
                used: 0x800,
                limit: 0x1000,
                peak: 0x900,
                resident: 0x400,
                soft_limit: 0x700,
                pressure: 50,
            },
            hk_mem::LedgerStats {
                capacity: hk_mem::hmp::CapacityId::new(9),
                kind: hk_mem::hmp::CapacityKind::System,
                used: 0x2000,
                limit: 0x4000,
                peak: 0x3000,
                resident: 0,
                soft_limit: 0,
                pressure: 50,
            },
        ],
        ..MockPlatform::new()
    };
    let written = Arc::clone(&platform.written);
    let mut runtime = runtime_on("b", platform);
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = hk_syscall::SYS_LEDGER_ENUM;
    context.rdi = 0x400;
    context.rsi = hk_syscall::LEDGER_RECORD_BYTES as u64;
    context.rdx = 0;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, hk_syscall::LEDGER_RECORD_BYTES as u64);
    let data = written.lock();
    assert_eq!(
        u64::from_le_bytes(data[..8].try_into().expect("capacity")),
        7
    );
    assert_eq!(
        u64::from_le_bytes(data[8..16].try_into().expect("used")),
        0x800
    );
    assert_eq!(
        u64::from_le_bytes(data[16..24].try_into().expect("limit")),
        0x1000
    );
    assert_eq!(
        u64::from_le_bytes(data[32..40].try_into().expect("pressure")),
        50
    );
    assert_eq!(
        u64::from_le_bytes(data[40..48].try_into().expect("resident")),
        0x400
    );
    assert_eq!(
        u64::from_le_bytes(data[48..56].try_into().expect("soft")),
        0x700
    );
    assert_eq!(
        u64::from_le_bytes(data[56..].try_into().expect("kind")),
        u64::from(hk_syscall::CAPACITY_KIND_USER)
    );
    drop(data);

    context.rax = hk_syscall::SYS_LEDGER_ENUM;
    context.rdx = 2;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0, "越界索引返回 0");
    let _ = runtime.finish(0, index);
}

#[test]
fn object_ledger_info_reads_handle_ledger() {
    let platform = MockPlatform::new();
    let written = Arc::clone(&platform.written);
    let mut runtime = runtime_on("b", platform);
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = SYS_CAPABILITY_CREATE;
    context.rdi = u64::from(hk_syscall::CapabilityKind::Anonymous.as_u8());
    context.rsi = 0x1000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;
    assert_ne!(capability, u64::MAX);

    context.rax = SYS_OBJECT_LEDGER;
    context.rdi = 0x400;
    context.rsi = 48;
    context.rdx = capability;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 48);
    let data = written.lock();
    assert_eq!(
        u64::from_le_bytes(data[..8].try_into().expect("charged")),
        0x1000
    );
    assert_eq!(
        u64::from_le_bytes(data[8..16].try_into().expect("limit")),
        0x1000
    );
    let _ = runtime.finish(0, index);
}

#[test]
fn waits_for_notification() {
    let mut runtime = runtime();

    let Step::Resume {
        index: waiter,
        mut context,
    } = runtime.step(0)
    else {
        panic!("任务应可运行");
    };
    assert_eq!(waiter, 0);
    context.rax = SYS_WAIT_NOTIFY;
    context.rdi = 1;
    context.rsi = 1;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Block);
    assert_eq!(runtime.tasks()[waiter].pending, "notify");

    let Step::Resume {
        index: signaler,
        mut context,
    } = runtime.step(0)
    else {
        panic!("任务应可运行");
    };
    assert_eq!(signaler, 1);
    context.rax = SYS_NOTIFY;
    context.rdi = 1;
    context.rsi = 1;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);

    let Step::Resume { index, context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    assert_eq!(index, waiter);
    assert_eq!(context.rax, 1);
    assert_eq!(runtime.tasks()[waiter].pending, "-");
}

#[test]
fn calls_and_replies() {
    let mut runtime = runtime();

    let Step::Resume {
        index: caller,
        mut context,
    } = runtime.step(0)
    else {
        panic!("任务应可运行");
    };
    assert_eq!(caller, 0);
    context.rax = SYS_CALL;
    context.rdi = 2;
    context.rsi = 9;
    context.rdx = 5;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Block);
    assert_eq!(runtime.tasks()[caller].pending, "call");

    let Step::Resume {
        index: server,
        mut context,
    } = runtime.step(0)
    else {
        panic!("任务应可运行");
    };
    assert_eq!(server, 1);
    context.rax = SYS_RECV;
    context.rdi = 2;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 9);
    assert_eq!(context.rdx, 5);
    assert_eq!(context.rcx, 1);

    context.rax = SYS_REPLY;
    context.rdi = 10;
    context.rsi = 6;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);

    let Step::Resume { index, context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    assert_eq!(index, caller);
    assert_eq!(context.rax, 10);
    assert_eq!(context.rdx, 6);
    assert_eq!(context.rcx, 2);
    assert_eq!(runtime.tasks()[caller].pending, "-");
}

#[test]
fn times_out_unanswered_call() {
    let mut runtime = runtime();

    let Step::Resume {
        index: caller,
        mut context,
    } = runtime.step(0)
    else {
        panic!("任务应可运行");
    };
    assert_eq!(caller, 0);
    context.rax = SYS_CALL_TIMEOUT;
    context.rdi = 2;
    context.rsi = 9;
    context.rdx = 5;
    context.r10 = 0;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Block);
    assert_eq!(runtime.tasks()[caller].pending, "call");

    // deadline 立即到期：wake_timeouts 取消挂起调用并唤醒调用方。
    let mut resumed = None;
    for _ in 0..6 {
        match runtime.step(0) {
            Step::Resume { index, context } if index == caller => {
                resumed = Some(context);
                break;
            }
            Step::Resume { mut context, .. } => {
                // 让服务端睡下，确保调用方获得调度。
                context.rax = SYS_SLEEP;
                context.rdi = 1;
                let _ = runtime.dispatch(0, &mut context);
            }
            _ => break,
        }
    }

    let context = resumed.expect("调用方应被超时唤醒");
    assert_eq!(context.rax, u64::MAX);
    assert_eq!(runtime.tasks()[caller].pending, "-");
}

#[test]
fn forks_task_with_child_context() {
    let mut runtime = runtime();
    let Step::Resume {
        index: parent,
        mut context,
    } = runtime.step(0)
    else {
        panic!("任务应可运行");
    };

    context.rax = SYS_FORK;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 3);
    assert_eq!(runtime.task_count(), 3);

    let tasks = runtime.tasks();
    assert_eq!(tasks[2].name, "a-fork");
    assert_eq!(tasks[2].parent, Some(tasks[parent].tcb));

    let mut steps = 0;
    loop {
        match runtime.step(0) {
            Step::Resume { index: 2, context } => {
                assert_eq!(context.rax, 0);
                break;
            }
            Step::Resume { .. } => {
                steps += 1;
                assert!(steps < 10);
            }
            _ => panic!("子任务应可运行"),
        }
    }
}

#[test]
fn attach_registers_loader_regions() {
    let mut runtime = runtime();
    let Step::Resume { index, context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let spec = UserRegionSpec {
        virt: 0x1000_0000,
        len: 0x2000,
        writable: false,
        executable: true,
    };
    runtime
        .attach_with_regions(index, context, &[spec])
        .expect("登记装载器 Region");
    let process = runtime.process(index);
    assert_eq!(process.hmp_arena.regions().len(), 1);
    assert_eq!(process.hmp_arena.used(), 0x2000);
    let region = process.regions.iter().flatten().next().expect("Region");
    assert_eq!(region.virt, spec.virt);
    assert_eq!(region.len, spec.len);
    assert!(region.rights.contains(Rights::EXECUTE));
    assert!(!region.rights.contains(Rights::WRITE));
}

#[test]
fn fork_cow_ranges_follow_regions() {
    let platform = MockPlatform::new();
    let last_cow = Arc::clone(&platform.last_cow);
    let mut runtime = runtime_on("b", platform);
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = SYS_CAPABILITY_CREATE_LAZY;
    context.rdi = u64::from(hk_syscall::CapabilityKind::Anonymous.as_u8());
    context.rsi = 0x1000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;

    context.rax = SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = 0x1000;
    context.r10 = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_ne!(context.rax, u64::MAX);
    let virt = runtime.process(index).regions[0]
        .as_ref()
        .expect("Region")
        .virt;

    context.rax = SYS_FORK;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_ne!(context.rax, u64::MAX);
    assert_eq!(*last_cow.lock(), alloc::vec![(virt, 0x1000)]);
    let _ = runtime.finish(0, index);
}

#[test]
fn threads_share_process_memory_state() {
    let mut runtime = runtime();
    let Step::Resume {
        index: leader,
        mut context,
    } = runtime.step(0)
    else {
        panic!("任务应可运行");
    };

    context.rax = SYS_THREAD_CREATE;
    context.rdi = 0x1000;
    context.rsi = 0x2000;
    context.rdx = 7;
    context.r10 = 0;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_ne!(context.rax, u64::MAX);
    let child = runtime.tasks.len() - 1;
    assert_eq!(runtime.tasks[child].process, runtime.tasks[leader].process);
    assert_eq!(
        runtime.process(child).arena_base,
        runtime.process(leader).arena_base
    );
    assert_eq!(
        runtime.process(child).ledger.limit(),
        runtime.process(leader).ledger.limit()
    );
}

#[test]
fn thread_fault_materializes_arena_page_through_process_capacity() {
    let mut runtime = runtime();
    let Step::Resume {
        index: leader,
        mut context,
    } = runtime.step(0)
    else {
        panic!("任务应可运行");
    };

    context.rax = SYS_ALLOC;
    context.rdi = 0x1000;
    context.rsi = 16;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let base = context.rax;
    assert_ne!(base, u64::MAX);

    context.rax = SYS_THREAD_CREATE;
    context.rdi = 0x1000;
    context.rsi = 0x2000;
    context.rdx = 7;
    context.r10 = 0;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_ne!(context.rax, u64::MAX);
    let child = runtime.tasks.len() - 1;

    // 线程的句柄表没有进程 Capacity；首次触碰 Arena 惰性页必须仍能装入。
    assert_eq!(base, runtime.process(leader).arena_base);
    runtime.current[0] = Some(child);
    assert!(runtime.anonymous_fault(0, base));
}

#[test]
fn activates_address_space_once_per_step() {
    let activations = Arc::new(AtomicUsize::new(0));
    let mut runtime = runtime_on(
        "b",
        MockPlatform {
            activations: Arc::clone(&activations),
            ..MockPlatform::new()
        },
    );

    let Step::Resume { index, .. } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    assert_eq!(activations.load(Ordering::Acquire), 1);
    let _ = runtime.finish(0, index);

    let _ = runtime.step(0);
    assert_eq!(activations.load(Ordering::Acquire), 2);
}

#[test]
fn schedules_on_multiple_cpus() {
    let mut runtime = runtime();
    runtime.set_online_cpus(2);
    let Step::Resume { index: first, .. } = runtime.step(0) else {
        panic!("BSP 应可调度");
    };
    let Step::Resume { index: second, .. } = runtime.step(1) else {
        panic!("AP 应可调度");
    };
    assert_ne!(first, second);
}

#[test]
fn user_fault_exits_current_task() {
    let mut runtime = runtime();
    let Step::Resume { index, .. } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    assert!(runtime.fault(0, 0xE00E));
    assert!(!runtime.fault(0, 0xE00E));
    assert_eq!(runtime.tasks()[index].state, "exited");
    assert_eq!(runtime.tasks()[index].handles, 0);
    assert_eq!(runtime.finish(0, index), Some((String::from("a"), 0xE00E)));
}

#[test]
fn exit_releases_peer_endpoint_handles() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let other = 1 - index;
    let before = runtime.tasks()[other].handles;
    assert_eq!(before, 5 + ARENA_HANDLES);

    context.rax = SYS_EXIT;
    context.rdi = 0;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Exit);
    assert!(runtime.finish(0, index).is_some());

    let tasks = runtime.tasks();
    assert_eq!(tasks[index].handles, 0);
    assert_eq!(tasks[other].handles, before - 2);

    let endpoint = runtime
        .start(
            "c",
            false,
            hk_cadence::DEFAULT_SPEC,
            MockContext::default(),
            None,
        )
        .expect("启动任务");
    assert_eq!(endpoint, 3);

    let tasks = runtime.tasks();
    assert_eq!(tasks[index].handles, 0);
    assert_eq!(tasks[other].handles, before);
    // 退出的端点与通知对象已释放，不再授予新任务；新任务另有初始 Capacity。
    assert_eq!(tasks[2].handles, 5 + ARENA_HANDLES);
}

#[test]
fn transports_byte_payloads() {
    use spin::Mutex;

    static DATA: Mutex<[u8; 8192]> = Mutex::new([0; 8192]);

    struct DataPlatform;

    impl UserPlatform for DataPlatform {
        fn timer_ticks(&self) -> u64 {
            0
        }

        fn pci_read(&self, _: u8, _: u8, _: u8, _: u8) -> u32 {
            0
        }

        fn zero(&self, _: u64, _: usize) {}

        fn arena_layout(&self, slot: usize) -> (u64, u64) {
            (0x10_0000 + slot as u64 * 0x2_0000, 0x1_0000)
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

        fn frame_bank_stats(&self) -> Option<hk_mem::FrameBankStats> {
            Some(hk_mem::FrameBankStats {
                total: 0x1000,
                used: 0x800,
                peak: 0x900,
                free: 0x800,
                by_capacity: [0x100, 0x200, 0x300, 0, 0],
            })
        }

        fn read_user(&self, address: u64, buffer: &mut [u8]) -> bool {
            let data = DATA.lock();
            let start = address as usize;
            let Some(end) = start.checked_add(buffer.len()) else {
                return false;
            };
            if end > data.len() {
                return false;
            }
            buffer.copy_from_slice(&data[start..end]);
            true
        }

        fn write_user(&self, address: u64, bytes: &[u8]) -> bool {
            let mut data = DATA.lock();
            let start = address as usize;
            let Some(end) = start.checked_add(bytes.len()) else {
                return false;
            };
            if end > data.len() {
                return false;
            }
            data[start..end].copy_from_slice(bytes);
            true
        }
    }

    let mut runtime = crate::boot::user_runtime(
        &[crate::ServiceSpec::new("a"), crate::ServiceSpec::new("b")],
        DataPlatform,
        |_, endpoint| MockContext {
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

    DATA.lock()[0x100..0x105].copy_from_slice(b"hello");
    let Step::Resume {
        index: sender,
        mut context,
    } = runtime.step(0)
    else {
        panic!("发送方应可运行");
    };
    assert_eq!(sender, 0);
    context.rax = SYS_SEND_DATA;
    context.rdi = 2;
    context.rsi = 0x400;
    context.rdx = 0x100;
    context.r10 = 5;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);

    let Step::Resume {
        index: receiver,
        mut context,
    } = runtime.step(0)
    else {
        panic!("接收方应可运行");
    };
    assert_eq!(receiver, 1);
    context.rax = SYS_RECV_DATA;
    context.rdi = 2;
    context.rsi = 0x200;
    context.rdx = 64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0x400);
    assert_eq!(context.r8, 5);
    assert_eq!(&DATA.lock()[0x200..0x205], b"hello");

    runtime.finish(0, receiver);
    DATA.lock()[0x100..0x110].copy_from_slice(b"0123456789abcdef");
    let Step::Resume {
        index: sender,
        mut context,
    } = runtime.step(0)
    else {
        panic!("发送方应可再次运行");
    };
    assert_eq!(sender, 0);
    context.rax = SYS_SEND_DATA;
    context.rdi = 2;
    context.rsi = 0x400;
    context.rdx = 0x100;
    context.r10 = 16;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    runtime.finish(0, sender);

    let Step::Resume {
        index: receiver,
        mut context,
    } = runtime.step(0)
    else {
        panic!("接收方应可再次运行");
    };
    assert_eq!(receiver, 1);
    context.rax = SYS_RECV_DATA;
    context.rdi = 2;
    context.rsi = 0x200;
    context.rdx = 8;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0x400);
    assert_eq!(context.r8, 8);
    assert_eq!(&DATA.lock()[0x200..0x208], b"01234567");

    context.rax = hk_syscall::SYS_ARENA_INFO;
    context.rdi = 0x300;
    context.rsi = 32;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 32);
    let data = DATA.lock();
    let base = u64::from_le_bytes(data[0x300..0x308].try_into().expect("base"));
    let size = u64::from_le_bytes(data[0x308..0x310].try_into().expect("size"));
    let regions = u64::from_le_bytes(data[0x318..0x320].try_into().expect("regions"));
    assert_ne!(base, 0, "Arena 应报告真实基址");
    assert!(size > 0);
    assert_eq!(regions, 0, "Arena 分配前不应有 Region");
    drop(data);

    context.rax = hk_syscall::SYS_FRAME_BANK_STATS;
    context.rdi = 0x400;
    context.rsi = 72;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 72);
    let data = DATA.lock();
    assert_eq!(
        u64::from_le_bytes(data[0x400..0x408].try_into().expect("total")),
        0x1000
    );
    assert_eq!(
        u64::from_le_bytes(data[0x408..0x410].try_into().expect("used")),
        0x800
    );
    assert_eq!(
        u64::from_le_bytes(data[0x410..0x418].try_into().expect("peak")),
        0x900
    );
    assert_eq!(
        u64::from_le_bytes(data[0x418..0x420].try_into().expect("free")),
        0x800
    );
    assert_eq!(
        u64::from_le_bytes(data[0x420..0x428].try_into().expect("system")),
        0x100
    );
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

    context.rax = hk_syscall::SYS_FRAME_SHARE;
    context.rdi = frame;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 2);
    context.rax = hk_syscall::SYS_FRAME_PIN;
    context.rdi = frame;
    context.rsi = 1;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);
    context.rax = hk_syscall::SYS_FRAME_PIN;
    context.rsi = 0;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);
    context.rax = hk_syscall::SYS_FRAME_RELEASE;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 1);

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

    context.rax = hk_syscall::SYS_CAPABILITY_CREATE_LAZY;
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
        .allocate_frame(tcb, capacity, hk_hal::PAGE_SIZE)
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

#[test]
fn lazy_anonymous_capability_maps_without_backing() {
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
    context.rsi = 0;
    context.rdx = 0x2000;
    context.r10 = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let region = context.rax;
    assert_ne!(region, u64::MAX);

    context.rax = SYS_REGION_INFO;
    context.rdi = region;
    context.rsi = 0;
    context.rdx = 16;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 16);

    assert!(runtime.anonymous_fault(0, hk_base::user::AUTO_WINDOW_BASE));
    let _ = runtime.finish(0, index);
}

#[test]
fn region_offset_maps_capability_page_at_region_start() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = SYS_CAPABILITY_CREATE_LAZY;
    context.rdi = 4;
    context.rsi = 0x4000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;
    assert_ne!(capability, u64::MAX);

    // 映射能力的第二个页（offset 0x1000），长度为 0x1000。
    context.rax = SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0x1000;
    context.rdx = 0x1000;
    context.r10 = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let region = context.rax;
    assert_ne!(region, u64::MAX);

    let virt = hk_base::user::AUTO_WINDOW_BASE;
    assert!(runtime.anonymous_fault(0, virt), "缺页应装入能力第二页");

    let map = runtime.process(0).regions.iter().flatten().next();
    let map = map.expect("Region 已登记");
    assert_eq!(map.offset, 0x1000, "Region 应保留能力内偏移");
    assert_eq!(map.virt, virt);
    let _ = runtime.finish(0, index);
}

#[test]
fn region_map_passes_rights_and_flags_to_page_table() {
    let platform = MockPlatform::new();
    let last_map = Arc::clone(&platform.last_map);
    let mut runtime = runtime_on("b", platform);
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = SYS_CAPABILITY_CREATE;
    context.rdi = 4;
    context.rsi = 0x2000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;
    assert_ne!(capability, u64::MAX);

    context.rax = SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = 0x2000;
    context.r10 = 0b11_u64 | (1_u64 << 32);
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_ne!(context.rax, u64::MAX);

    let packed = last_map.load(Ordering::Acquire);
    let rights = hk_handle::Rights::from_bits_truncate(packed as u32);
    let flags = hk_mem::hmp::RegionFlags::from_bits_truncate((packed >> 32) as u32);
    assert_eq!(rights, hk_handle::Rights::READ | hk_handle::Rights::WRITE);
    assert!(flags.contains(hk_mem::hmp::RegionFlags::COW));
    let _ = runtime.finish(0, index);
}

#[test]
fn region_map_rejects_rights_not_granted_by_handle() {
    let platform = MockPlatform::new();
    let mut runtime = runtime_on("b", platform);
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let tcb = runtime.tasks[index].tcb;

    context.rax = SYS_CAPABILITY_CREATE;
    context.rdi = 4;
    context.rsi = 0x2000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;
    assert_ne!(capability, u64::MAX);
    let (handle, _, _) = runtime
        .capability_at(index, capability as usize)
        .expect("capability");
    let readonly = runtime
        .kernel
        .duplicate_handle(tcb, handle, hk_handle::Rights::READ)
        .expect("只读副本");
    let extra = runtime
        .install_extra_capability(index, readonly)
        .expect("安装只读句柄");

    context.rax = SYS_REGION_MAP;
    context.rdi = extra;
    context.rsi = 0;
    context.rdx = 0x1000;
    context.r10 = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX, "只读句柄不能映射可写");

    context.rax = SYS_REGION_MAP;
    context.r10 = 0b1;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_ne!(context.rax, u64::MAX, "只读映射仍应允许");
    let _ = runtime.finish(0, index);
}

#[test]
fn region_protect_updates_rights_and_page_table() {
    let platform = MockPlatform::new();
    let last_map = Arc::clone(&platform.last_map);
    let mut runtime = runtime_on("b", platform);
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = SYS_CAPABILITY_CREATE;
    context.rdi = u64::from(hk_syscall::CapabilityKind::Frame.as_u8());
    context.rsi = 0x1000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;

    context.rax = SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = 0x1000;
    context.r10 = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let region = context.rax;
    assert_ne!(region, u64::MAX);

    last_map.store(0, Ordering::Release);
    context.rax = SYS_REGION_PROTECT;
    context.rdi = region;
    context.rsi = 0b101;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);

    let packed = last_map.load(Ordering::Acquire);
    let rights = hk_handle::Rights::from_bits_truncate(packed as u32);
    assert_eq!(
        rights,
        hk_handle::Rights::READ | hk_handle::Rights::EXECUTE,
        "protect 应把页表切换为只读可执行"
    );

    let map = runtime.process(0).regions.iter().flatten().next().unwrap();
    assert_eq!(map.rights, rights);
    let _ = runtime.finish(0, index);
}

#[test]
fn region_protect_updates_lazy_pages() {
    let platform = MockPlatform::new();
    let last_map = Arc::clone(&platform.last_map);
    let mut runtime = runtime_on("b", platform);
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    let _capability = create_lazy_region(&mut runtime, &mut context, 0x2000);
    assert!(runtime.anonymous_fault(0, hk_base::user::AUTO_WINDOW_BASE));
    last_map.store(0, Ordering::Release);

    context.rax = SYS_REGION_PROTECT;
    context.rdi = 0x4001;
    context.rsi = 0b1;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);

    let packed = last_map.load(Ordering::Acquire);
    let rights = hk_handle::Rights::from_bits_truncate(packed as u32);
    assert_eq!(
        rights,
        hk_handle::Rights::READ,
        "惰性 Region 的已装入页也必须更新 PTE"
    );
    let _ = runtime.finish(0, index);
}

#[test]
fn cow_resolution_records_private_page_override() {
    let platform = MockPlatform {
        cow: Some(CowOutcome::Copied(0x7000_0000)),
        ..MockPlatform::new()
    };
    let mut runtime = runtime_on("b", platform);
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let _capability = create_lazy_region(&mut runtime, &mut context, 0x2000);
    assert!(runtime.anonymous_fault(0, hk_base::user::AUTO_WINDOW_BASE));
    assert!(runtime.resolve_cow(0, hk_base::user::AUTO_WINDOW_BASE));

    assert!(
        runtime
            .process(index)
            .cow_pages
            .values()
            .any(|range| range.start().get() == 0x7000_0000),
        "COW 复制页必须登记为进程私有覆盖"
    );
    runtime.teardown_process(index);
    assert!(
        runtime.process(index).cow_pages.is_empty(),
        "拆除 Region 后应清理覆盖表"
    );
    let _ = runtime.finish(0, index);
}

#[test]
fn fork_copies_cow_private_page_overrides() {
    let platform = MockPlatform {
        cow: Some(CowOutcome::Copied(0x7000_1000)),
        ..MockPlatform::new()
    };
    let mut runtime = runtime_on("b", platform);
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let _capability = create_lazy_region(&mut runtime, &mut context, 0x2000);
    assert!(runtime.anonymous_fault(0, hk_base::user::AUTO_WINDOW_BASE));
    assert!(runtime.resolve_cow(0, hk_base::user::AUTO_WINDOW_BASE));
    assert_eq!(
        runtime.process(index).regions.iter().flatten().count(),
        1,
        "父 Region"
    );

    context.rax = SYS_FORK;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_ne!(context.rax, u64::MAX);
    let child = runtime.task_count() - 1;
    assert_eq!(
        runtime.process(child).regions.iter().flatten().count(),
        1,
        "子 Region"
    );
    assert!(
        runtime
            .process(child)
            .cow_pages
            .values()
            .any(|range| range.start().get() == 0x7000_1000),
        "fork 必须复制进程级覆盖表"
    );
    let _ = runtime.finish(0, index);
}

#[test]
fn exit_tears_down_regions_and_releases_vspace() {
    let platform = MockPlatform::new();
    let unmapped = Arc::clone(&platform.unmapped);
    let released = Arc::clone(&platform.released);
    let mut runtime = runtime_on("b", platform);
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let _capability = create_lazy_region(&mut runtime, &mut context, 0x2000);
    assert!(runtime.anonymous_fault(0, hk_base::user::AUTO_WINDOW_BASE));

    context.rax = SYS_EXIT;
    context.rdi = 0;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Exit);
    assert!(runtime.finish(0, index).is_some());
    assert_eq!(
        runtime.process(index).regions.iter().flatten().count(),
        0,
        "退出时必须拆除 Region"
    );
    assert!(
        unmapped.load(core::sync::atomic::Ordering::Acquire) > 0,
        "退出时必须解除用户映射"
    );
    assert!(
        released.load(core::sync::atomic::Ordering::Acquire) > 0,
        "退出时必须释放地址空间"
    );
}

#[test]
fn guard_regions_fault_without_install() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    context.rax = SYS_CAPABILITY_CREATE_LAZY;
    context.rdi = 4;
    context.rsi = 0x1000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;
    assert_ne!(capability, u64::MAX);

    context.rax = SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = 0x1000;
    context.r10 = 0b11 | ((hk_mem::hmp::RegionFlags::GUARD.bits() as u64) << 32);
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_ne!(context.rax, u64::MAX);

    assert!(
        !runtime.anonymous_fault(0, hk_base::user::AUTO_WINDOW_BASE),
        "GUARD 页不能装入零页"
    );
    let _ = runtime.finish(0, index);
}

#[test]
fn dont_fork_regions_are_not_copied() {
    let mut runtime = runtime();
    let Step::Resume {
        index: parent,
        mut context,
    } = runtime.step(0)
    else {
        panic!("任务应可运行");
    };
    context.rax = SYS_CAPABILITY_CREATE_LAZY;
    context.rdi = 4;
    context.rsi = 0x1000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;
    assert_ne!(capability, u64::MAX);

    context.rax = SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = 0x1000;
    context.r10 = 0b11 | ((hk_mem::hmp::RegionFlags::DONT_FORK.bits() as u64) << 32);
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_ne!(context.rax, u64::MAX);

    context.rax = SYS_FORK;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_ne!(context.rax, u64::MAX);
    let child = runtime.task_count() - 1;
    assert_eq!(
        runtime.process(parent).regions.iter().flatten().count(),
        1,
        "父任务保留 Region"
    );
    assert_eq!(
        runtime.process(child).regions.iter().flatten().count(),
        0,
        "DONT_FORK Region 不能复制到子任务"
    );
    let _ = runtime.finish(0, parent);
}

#[test]
fn region_protect_rejects_wx_conflict() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = SYS_CAPABILITY_CREATE_LAZY;
    context.rdi = 4;
    context.rsi = 0x1000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;

    context.rax = SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = 0x1000;
    context.r10 = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let region = context.rax;

    context.rax = SYS_REGION_PROTECT;
    context.rdi = region;
    context.rsi = 0b111;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX, "W^X 冲突应被拒绝");
    let _ = runtime.finish(0, index);
}

#[test]
fn region_map_rejects_execute_on_non_executable_capability() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    context.rax = SYS_CAPABILITY_CREATE_LAZY;
    context.rdi = u64::from(hk_syscall::CapabilityKind::Anonymous.as_u8());
    context.rsi = 0x1000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;

    context.rax = SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = 0x1000;
    context.r10 = 0b101;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX, "Anonymous 不能映射为可执行");
    let _ = runtime.finish(0, index);
}

#[test]
fn provider_kind_must_match_capability() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    context.rax = SYS_CAPABILITY_CREATE_LAZY;
    context.rdi = u64::from(hk_syscall::CapabilityKind::Surface.as_u8());
    context.rsi = 0x1000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;

    context.rax = SYS_CAPABILITY_BIND_PROVIDER;
    context.rdi = capability;
    context.rsi = 2;
    context.rdx = u64::from(hk_syscall::PROVIDER_KIND_ANONYMOUS);
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX, "Provider 类型必须匹配能力类型");

    context.rax = SYS_CAPABILITY_BIND_PROVIDER;
    context.rdx = u64::from(hk_syscall::PROVIDER_KIND_SURFACE);
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);
    let _ = runtime.finish(0, index);
}

#[test]
fn region_grow_requires_anonymous_capability() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    context.rax = SYS_CAPABILITY_CREATE;
    context.rdi = u64::from(hk_syscall::CapabilityKind::Surface.as_u8());
    context.rsi = 0x2000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;

    context.rax = SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = 0x1000;
    context.r10 = 0b11 | ((hk_mem::hmp::RegionFlags::GROW.bits() as u64) << 32);
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let region = context.rax;
    assert_ne!(region, u64::MAX);

    context.rax = SYS_REGION_GROW;
    context.rdi = region;
    context.rsi = 0x2000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX, "只有 Anonymous 能力可以增长");
    let _ = runtime.finish(0, index);
}

#[test]
fn capability_phys_requires_resident_kind() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    context.rax = SYS_CAPABILITY_CREATE;
    context.rdi = u64::from(hk_syscall::CapabilityKind::Revision.as_u8());
    context.rsi = 0x1000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let revision = context.rax;

    context.rax = hk_syscall::SYS_CAPABILITY_PHYS;
    context.rdi = revision;
    context.rsi = 0x2000;
    context.rdx = 16;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX, "Revision 不提供 PHYS 查询");

    context.rax = SYS_CAPABILITY_CREATE;
    context.rdi = u64::from(hk_syscall::CapabilityKind::Surface.as_u8());
    context.rsi = 0x1000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let surface = context.rax;
    context.rax = hk_syscall::SYS_CAPABILITY_PHYS;
    context.rdi = surface;
    context.rsi = 0x2000;
    context.rdx = 16;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 16);
    let _ = runtime.finish(0, index);
}

#[test]
fn capacity_merge_returns_child_range_to_parent() {
    let mut runtime = runtime_with(hk_syscall::MEMORY_SERVICE);
    let Step::Resume {
        index: first,
        mut context,
    } = runtime.step(0)
    else {
        panic!("任务应可运行");
    };
    // 让第一个任务阻塞，接着调度到持有 User Capacity 的 memory 服务。
    context.rax = SYS_RECV;
    context.rdi = 1;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Block);

    let Step::Resume {
        index: memory,
        mut context,
    } = runtime.step(0)
    else {
        panic!("memory 服务应可运行");
    };
    assert_eq!(memory, 1);
    assert_ne!(first, memory);

    let parent = 0x5001;
    context.rax = SYS_CAPACITY_INFO;
    context.rdi = parent;
    context.rsi = 0x4000;
    context.rdx = 64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 64);

    context.rax = SYS_CAPACITY_SPLIT;
    context.rdi = parent;
    context.rsi = 0x4000;
    context.rdx = hk_syscall::CAPACITY_KIND_USER as u64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let child = context.rax;
    assert_ne!(child, u64::MAX, "应切出子 Capacity");

    context.rax = SYS_CAPACITY_MERGE;
    context.rdi = parent;
    context.rsi = child;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0, "并回应成功");

    // 子句柄已关闭，再用它应失败。
    context.rax = SYS_CAPACITY_INFO;
    context.rdi = child;
    context.rsi = 0x4000;
    context.rdx = 64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX, "合并后子句柄已失效");
    let _ = runtime.finish(0, memory);
}

#[test]
fn arena_tracks_mapped_regions() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = SYS_CAPABILITY_CREATE_LAZY;
    context.rdi = 4;
    context.rsi = 0x2000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;

    context.rax = SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = 0x2000;
    context.r10 = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let region = context.rax;
    assert_ne!(region, u64::MAX);
    let arena = &runtime.process(0).hmp_arena;
    assert_eq!(arena.regions().len(), 1, "用户 Region 应登记进 Arena");
    assert!(arena.used() >= 0x2000);

    context.rax = SYS_REGION_UNMAP;
    context.rdi = region;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 1);
    assert_eq!(
        runtime.process(0).hmp_arena.regions().len(),
        0,
        "解除后 Arena 表应为空"
    );
    let _ = runtime.finish(0, index);
}

#[test]
fn grows_grow_region_and_extends_arena() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = SYS_CAPABILITY_CREATE_LAZY;
    context.rdi = 4;
    context.rsi = 0x2000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;

    context.rax = SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = 0x1000;
    context.r10 = 0b11 | ((hk_mem::hmp::RegionFlags::GROW.bits() as u64) << 32);
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let region = context.rax;
    assert_ne!(region, u64::MAX);
    let charged = runtime.process(index).ledger.charged();

    context.rax = SYS_REGION_GROW;
    context.rdi = region;
    context.rsi = 0x2000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0, "GROW Region 应可扩展");
    assert_eq!(
        runtime.process(index).ledger.charged(),
        charged,
        "GROW 扩展的是已计费能力，不能重复收费"
    );
    assert_eq!(
        runtime.region_at(index, region as usize).expect("Region").3,
        0x2000
    );
    assert!(
        runtime
            .process(index)
            .hmp_arena
            .regions()
            .iter()
            .any(|entry| entry.len() == 0x2000),
        "Arena 表应同步扩展"
    );
    let _ = runtime.finish(0, index);
}

#[test]
fn pinned_region_is_skipped_by_reclaim() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = SYS_CAPABILITY_CREATE_LAZY;
    context.rdi = 4;
    context.rsi = 0x1000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;
    context.rax = SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = 0x1000;
    context.r10 = 0b11 | ((hk_mem::hmp::RegionFlags::PINNED.bits() as u64) << 32);
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let region = context.rax;
    let (_, _, virt, _) = runtime.region_at(index, region as usize).expect("Region");
    assert!(runtime.anonymous_fault(0, virt));
    assert_eq!(
        runtime.reclaim_zero_pages(index, hk_hal::PAGE_SIZE as u64),
        0,
        "PINNED Region 的页不能被回收"
    );
    let _ = runtime.finish(0, index);
}

#[test]
fn lazy_region_unmap_failure_keeps_region() {
    let mut runtime = runtime_on(
        "b",
        MockPlatform {
            unmap_ok: false,
            ..MockPlatform::new()
        },
    );
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = SYS_CAPABILITY_CREATE_LAZY;
    context.rdi = 4;
    context.rsi = 0x2000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;

    context.rax = SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = 0x2000;
    context.r10 = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let region = context.rax;
    assert_ne!(region, u64::MAX);

    context.rax = SYS_REGION_UNMAP;
    context.rdi = region;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);
    assert_eq!(
        runtime.process(0).hmp_arena.regions().len(),
        1,
        "解除失败时用户 Region 仍登记"
    );
    assert_eq!(runtime.process(0).regions.iter().flatten().count(), 1);
    let _ = runtime.finish(0, index);
}

#[test]
fn provider_fault_blocks_caller() {
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

    assert!(runtime.provider_fault(0, hk_base::user::AUTO_WINDOW_BASE, MockContext::default()));
    assert_eq!(runtime.tasks()[0].pending, "provider-fault");

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
    assert_ne!(frame, u64::MAX);

    context.rax = hk_syscall::SYS_REPLY_HANDLE;
    context.rdi = hk_syscall::PROVIDER_PROVIDE;
    context.rsi = 0;
    context.rdx = frame;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);

    let Step::Resume { index, context } = runtime.step(0) else {
        panic!("缺页任务应恢复");
    };
    assert_eq!(index, 0);
    assert_eq!(context.rax, 0);
    let _ = runtime.finish(0, index);
}

#[test]
fn anonymous_fault_defers_to_bound_provider() {
    let mut runtime = runtime_with(hk_syscall::MEMORY_SERVICE);
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = SYS_CAPABILITY_CREATE_LAZY;
    context.rdi = 4;
    context.rsi = 0x2000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;

    context.rax = SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = 0x2000;
    context.r10 = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);

    context.rax = SYS_CAPABILITY_BIND_PROVIDER;
    context.rdi = capability;
    context.rsi = 2;
    context.rdx = u64::from(hk_syscall::PROVIDER_KIND_ANONYMOUS);
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);

    assert!(
        !runtime.anonymous_fault(0, hk_base::user::AUTO_WINDOW_BASE),
        "绑定 Provider 后不进入内核零页快速路径"
    );
    assert!(runtime.provider_fault(0, hk_base::user::AUTO_WINDOW_BASE, MockContext::default()));
    let _ = runtime.finish(0, index);
}

#[test]
fn ledger_limit_rejects_over_quota_and_releases_on_close() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = SYS_CAPABILITY_CREATE_LAZY;
    context.rdi = 4;
    context.rsi = MEMORY_LIMIT;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;
    assert_ne!(capability, u64::MAX);

    context.rax = SYS_CAPABILITY_CREATE_LAZY;
    context.rdi = 4;
    context.rsi = 0x1000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX, "超出配额应被拒绝");

    context.rax = SYS_HANDLE_CLOSE;
    context.rdi = capability;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);

    context.rax = SYS_CAPABILITY_CREATE_LAZY;
    context.rdi = 4;
    context.rsi = MEMORY_LIMIT;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_ne!(context.rax, u64::MAX, "关闭后配额应归还");
    let _ = runtime.finish(0, index);
}

#[test]
fn closing_capability_unmaps_its_regions() {
    let platform = MockPlatform::new();
    let mut runtime = runtime_on("b", platform);
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = SYS_CAPABILITY_CREATE;
    context.rdi = 4;
    context.rsi = 0x2000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;
    assert_ne!(capability, u64::MAX);

    context.rax = SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = 0x2000;
    context.r10 = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_ne!(context.rax, u64::MAX);

    context.rax = SYS_HANDLE_CLOSE;
    context.rdi = capability;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);
    assert!(
        runtime.process(0).regions.iter().flatten().next().is_none(),
        "关闭 capability 前必须先解除基于它的 Region"
    );
    let _ = runtime.finish(0, index);
}

#[test]
fn scanout_requires_surface_and_holds_pin() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = SYS_CAPABILITY_CREATE;
    context.rdi = 1;
    context.rsi = 0x1000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let surface = context.rax;
    assert_ne!(surface, u64::MAX);

    context.rax = SYS_CAPABILITY_SCANOUT;
    context.rdi = surface;
    context.rsi = 1;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0, "Surface 可以进入 scanout 驻留");

    context.rax = SYS_CAPABILITY_PIN;
    context.rdi = surface;
    context.rsi = 0;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX, "scanout 期间不能解除固定");

    context.rax = SYS_CAPABILITY_SCANOUT;
    context.rdi = surface;
    context.rsi = 0;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);

    context.rax = SYS_CAPABILITY_PIN;
    context.rdi = surface;
    context.rsi = 0;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0, "退出 scanout 后可解除固定");

    context.rax = SYS_CAPABILITY_CREATE_LAZY;
    context.rdi = 4;
    context.rsi = 0x1000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let anonymous = context.rax;
    assert_ne!(anonymous, u64::MAX);
    context.rax = SYS_CAPABILITY_SCANOUT;
    context.rdi = anonymous;
    context.rsi = 1;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX, "匿名能力不能 scanout");

    let _ = runtime.finish(0, index);
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

    fn map_user(
        &self,
        _slot: usize,
        _virt: u64,
        _phys: u64,
        _len: usize,
        _rights: hk_handle::Rights,
        _flags: hk_mem::hmp::RegionFlags,
    ) -> bool {
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

    fn fork(&self, _parent: usize, _child: usize, _cow_ranges: &[(u64, u64)]) -> bool {
        true
    }

    fn share_vspace(&self, _parent: usize, _child: usize) -> bool {
        true
    }

    fn write_user(&self, _address: u64, _data: &[u8]) -> bool {
        true
    }
}

#[test]
fn reclaim_skips_shared_pages() {
    let platform = MockPlatform {
        shared: true,
        ..MockPlatform::new()
    };
    let mut runtime = runtime_on("b", platform);
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    create_lazy_region(&mut runtime, &mut context, 0x2000);
    assert!(runtime.anonymous_fault(0, hk_base::user::AUTO_WINDOW_BASE));
    assert_eq!(runtime.reclaim_zero_pages(0, 0x1000), 0, "共享页不能回收");
    let _ = runtime.finish(0, index);
}

#[test]
fn reclaims_zero_pages_and_reloads_them() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    create_lazy_region(&mut runtime, &mut context, 0x2000);
    assert!(runtime.anonymous_fault(0, hk_base::user::AUTO_WINDOW_BASE));
    assert!(runtime.anonymous_fault(0, hk_base::user::AUTO_WINDOW_BASE + 4096));

    assert_eq!(runtime.reclaim_zero_pages(0, 0x2000), 0x2000);
    assert_eq!(runtime.reclaim_zero_pages(0, 0x2000), 0, "回收后页已归还");

    assert!(
        runtime.anonymous_fault(0, hk_base::user::AUTO_WINDOW_BASE),
        "回收后再次缺页可重新装入"
    );
    let _ = runtime.finish(0, index);
}

#[test]
fn reclaim_skips_non_zero_pages() {
    let mut runtime = runtime_on(
        "b",
        MockPlatform {
            zero: false,
            ..MockPlatform::new()
        },
    );
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    create_lazy_region(&mut runtime, &mut context, 0x1000);
    assert!(runtime.anonymous_fault(0, hk_base::user::AUTO_WINDOW_BASE));

    assert_eq!(runtime.reclaim_zero_pages(0, 0x1000), 0);
    let _ = runtime.finish(0, index);
}

#[test]
fn allocation_pressure_reclaims_zero_pages() {
    let platform = ShortPlatform {
        windows: core::sync::atomic::AtomicUsize::new(3),
    };
    let mut runtime = runtime_on("b", platform);
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    create_lazy_region(&mut runtime, &mut context, 0x4000);
    for page in 0..3 {
        assert!(runtime.anonymous_fault(0, hk_base::user::AUTO_WINDOW_BASE + page * 4096));
    }

    assert!(
        runtime.anonymous_fault(0, hk_base::user::AUTO_WINDOW_BASE + 3 * 4096),
        "池耗尽后回收零页继续装入"
    );
    assert!(runtime.anonymous_fault(0, hk_base::user::AUTO_WINDOW_BASE));
    let _ = runtime.finish(0, index);
}
