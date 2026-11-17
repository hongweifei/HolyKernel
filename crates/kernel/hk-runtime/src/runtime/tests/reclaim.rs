//! 零页回收、System Arena 与能力换页测试。

use super::*;

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

/// 提供固定 System Arena 快照的测试平台。
struct SystemArenaPlatform {
    inner: MockPlatform,
}

impl UserPlatform for SystemArenaPlatform {
    fn timer_ticks(&self) -> u64 {
        self.inner.timer_ticks()
    }

    fn pci_read(&self, bus: u8, device: u8, function: u8, offset: u8) -> u32 {
        self.inner.pci_read(bus, device, function, offset)
    }

    fn zero(&self, address: u64, length: usize) {
        self.inner.zero(address, length);
    }

    fn arena_layout(&self, slot: usize) -> (u64, u64) {
        self.inner.arena_layout(slot)
    }

    fn user_capacity(&self) -> Option<Arc<spin::Mutex<hk_mem::hmp::Capacity>>> {
        self.inner.user_capacity()
    }

    fn write_user(&self, address: u64, data: &[u8]) -> bool {
        self.inner.write_user(address, data)
    }

    fn system_arena_info(&self) -> Option<crate::runtime::SystemArenaInfo> {
        Some(crate::runtime::SystemArenaInfo {
            base: 0xFFFF_8000_0000_0000,
            size: 0x0000_8000_0000_0000,
            used: 0x3000,
            regions: 2,
        })
    }

    fn system_arena_region(&self, index: usize) -> Option<crate::runtime::SystemRegionRecord> {
        match index {
            0 => Some(crate::runtime::SystemRegionRecord {
                virt: 0xFFFF_8000_0000_0000,
                len: 0x2000,
                rights: hk_handle::Rights::READ.bits() | hk_handle::Rights::WRITE.bits(),
                flags: hk_mem::hmp::RegionFlags::PINNED.bits(),
            }),
            1 => Some(crate::runtime::SystemRegionRecord {
                virt: 0xFFFF_8000_0000_2000,
                len: 0x1000,
                rights: hk_handle::Rights::READ.bits() | hk_handle::Rights::EXECUTE.bits(),
                flags: hk_mem::hmp::RegionFlags::PINNED.bits()
                    | hk_mem::hmp::RegionFlags::SEALED.bits(),
            }),
            _ => None,
        }
    }
}

#[test]
fn system_arena_info_and_regions_are_enumerable() {
    let platform = SystemArenaPlatform {
        inner: MockPlatform::new(),
    };
    let mut runtime = runtime_on("b", platform);
    let Step::Resume { mut context, .. } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    // 只有 Debug/Checkpoint 服务可以枚举内核映射。
    runtime.tasks[0].name = String::from(hk_syscall::DEBUG_SERVICE);

    context.rax = hk_syscall::SYS_SYSTEM_ARENA_INFO;
    context.rdi = 0x500;
    context.rsi = 32;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 32);
    let written = runtime.platform.inner.written.lock();
    assert_eq!(
        u64::from_le_bytes(written[0..8].try_into().expect("base")),
        0xFFFF_8000_0000_0000
    );
    assert_eq!(
        u64::from_le_bytes(written[16..24].try_into().expect("used")),
        0x3000
    );
    assert_eq!(
        u64::from_le_bytes(written[24..32].try_into().expect("regions")),
        2
    );
    drop(written);

    // out == 0 时只返回 Region 数。
    context.rax = hk_syscall::SYS_SYSTEM_ARENA_REGIONS;
    context.rdi = 0;
    context.rsi = 0;
    context.rdx = 0;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 2);

    context.rax = hk_syscall::SYS_SYSTEM_ARENA_REGIONS;
    context.rdi = 1;
    context.rsi = 0x600;
    context.rdx = hk_syscall::SYSTEM_REGION_RECORD_BYTES as u64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, hk_syscall::SYSTEM_REGION_RECORD_BYTES as u64);
    let written = runtime.platform.inner.written.lock();
    assert_eq!(
        u64::from_le_bytes(written[0..8].try_into().expect("virt")),
        0xFFFF_8000_0000_2000
    );
    assert_eq!(
        u32::from_le_bytes(written[16..20].try_into().expect("rights")),
        hk_handle::Rights::READ.bits() | hk_handle::Rights::EXECUTE.bits()
    );
    drop(written);

    // 越界索引返回失败。
    context.rax = hk_syscall::SYS_SYSTEM_ARENA_REGIONS;
    context.rdi = 2;
    context.rsi = 0x600;
    context.rdx = hk_syscall::SYSTEM_REGION_RECORD_BYTES as u64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX);
}

#[test]
fn system_arena_enumeration_requires_debug_service() {
    let mut runtime = runtime();
    let Step::Resume { mut context, .. } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = hk_syscall::SYS_SYSTEM_ARENA_INFO;
    context.rdi = 0x500;
    context.rsi = 32;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX, "普通服务不得枚举 System Arena");

    context.rax = hk_syscall::SYS_SYSTEM_ARENA_REGIONS;
    context.rdi = 0;
    context.rsi = 0;
    context.rdx = 0;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX, "普通服务不得枚举 System Region");
}

#[test]
fn system_arena_queries_fail_without_platform_support() {
    let mut runtime = runtime();
    let Step::Resume { mut context, .. } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    context.rax = hk_syscall::SYS_SYSTEM_ARENA_INFO;
    context.rdi = 0x500;
    context.rsi = 32;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX);

    context.rax = hk_syscall::SYS_SYSTEM_ARENA_REGIONS;
    context.rdi = 0;
    context.rsi = 0;
    context.rdx = 0;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX);
}

#[test]
fn resident_capability_page_maps_without_provider() {
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
    context.rdi = u64::from(hk_mem::hmp::CapabilityKind::Anonymous.as_u8());
    context.rsi = 0x2000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let capability = context.rax;
    assert_ne!(capability, u64::MAX);
    runtime
        .kernel
        .bind_capability_provider(
            tcb,
            runtime
                .handle_at(index, capability as usize)
                .expect("能力句柄"),
            endpoint,
            hk_mem::hmp::ProviderKind::Anonymous,
        )
        .expect("绑定 Provider");

    context.rax = hk_syscall::SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = 0x1000;
    context.r10 = u64::from(hk_handle::Rights::READ.bits());
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let region = context.rax;
    assert_ne!(region, u64::MAX);
    let (_, _, virt, _) = runtime.region_at(index, region as usize).expect("Region");

    runtime
        .kernel
        .set_capability_page(
            tcb,
            runtime
                .handle_at(index, capability as usize)
                .expect("能力句柄"),
            0,
            hk_mem::hmp::FrameBacking::Imported(hk_mem::hmp::FrameLease::import(
                hk_hal::PhysAddr::new(0x70_0000),
                hk_hal::PAGE_SIZE,
            )),
        )
        .expect("装入页");

    assert_eq!(
        runtime.handle_fault(
            0,
            virt,
            hk_mem::hmp::provider::ProviderAccess::Read,
            MockContext::default(),
        ),
        FaultOutcome::Resumed,
        "已常驻能力页应直接映射，不应转发 Provider"
    );
    assert!(runtime.platform.user_mapped(virt));
    let _ = runtime.finish(0, index);
}

#[test]
fn arena_grow_extends_alloc_window() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let initial = runtime.process(index).arena_size;
    assert!(initial >= hk_hal::PAGE_SIZE as u64);

    context.rax = hk_syscall::SYS_ARENA_GROW;
    context.rdi = hk_hal::PAGE_SIZE as u64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, initial + hk_hal::PAGE_SIZE as u64);

    context.rax = hk_syscall::SYS_ARENA_GROW;
    context.rdi = 0;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, initial + hk_hal::PAGE_SIZE as u64);

    // 原先会超出窗口的分配，在增长后应成功。
    let size = (initial as usize).saturating_add(hk_hal::PAGE_SIZE);
    assert!(runtime.process_mut(index).allocate_arena(size, 8).is_some());
    let _ = runtime.finish(0, index);
}

#[test]
fn swap_without_provider_keeps_page() {
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
    context.rax = hk_syscall::SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = 0x1000;
    context.r10 = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let region = context.rax;
    let (_, _, virt, _) = runtime.region_at(index, region as usize).expect("Region");
    assert!(runtime.anonymous_fault(0, virt));

    context.rax = hk_syscall::SYS_CAPABILITY_SWAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = hk_hal::PAGE_SIZE as u64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX);
    let (handle, _, _) = runtime
        .capability_at(index, capability as usize)
        .expect("能力");
    assert!(
        runtime
            .kernel
            .capability_page(tcb, handle, 0)
            .expect("查询")
            .is_some(),
        "没有 Provider 时不得由内核保存页"
    );
    let _ = runtime.finish(0, index);
}
