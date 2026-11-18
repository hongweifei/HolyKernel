//! Region 映射、权限、COW 与 fork 测试。

use super::*;

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
fn resident_capability_backing_is_zeroed() {
    let platform = MockPlatform::new();
    let physical = Arc::clone(&platform.phys);
    let mut runtime = runtime_on("b", platform);
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = SYS_CAPABILITY_CREATE;
    context.rdi = hk_syscall::CapabilityKind::Anonymous.as_u8() as u64;
    context.rsi = 0x2000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_ne!(context.rax, u64::MAX);
    let memory = physical.lock();
    assert!(memory.len() >= 0x2000, "常驻能力创建必须写入整段清零页");
    assert!(
        memory.values().all(|byte| *byte == 0),
        "常驻能力首次可读必须为零"
    );
    let _ = runtime.finish(0, index);
}

#[test]
fn region_handles_are_not_reused_after_unmap() {
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
    context.rdx = 0x1000;
    context.r10 = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let first = context.rax;
    assert_ne!(first, u64::MAX);

    context.rax = SYS_REGION_UNMAP;
    context.rdi = first;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);

    context.rax = SYS_REGION_MAP;
    context.rdi = capability;
    context.rsi = 0;
    context.rdx = 0x1000;
    context.r10 = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let second = context.rax;
    assert_ne!(second, u64::MAX);
    assert_ne!(first, second, "槽位复用必须更换用户句柄");

    // 旧句柄不得命中复用后的新 Region。
    context.rax = SYS_REGION_INFO;
    context.rdi = first;
    context.rsi = 0;
    context.rdx = 16;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX, "陈旧 Region 句柄必须失效");

    context.rax = SYS_REGION_INFO;
    context.rdi = second;
    context.rsi = 0;
    context.rdx = 16;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 16);
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
fn region_protect_cannot_exceed_capability_rights() {
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
    context.r10 = 0b1;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let region = context.rax;
    assert_ne!(region, u64::MAX);

    // 只读能力句柄不能被 protect 提升为可写映射。
    context.rax = SYS_REGION_PROTECT;
    context.rdi = region;
    context.rsi = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX, "protect 不能超过能力句柄权限");
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
    // fork 后的 Region 才是 COW；显式标记后再触发写时复制。
    context.rax = SYS_REGION_PROTECT;
    context.rdi = 0x4001;
    context.rsi = 0b11 | ((hk_mem::hmp::RegionFlags::COW.bits() as u64) << 32);
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);
    assert!(runtime.resolve_cow(0, hk_base::user::AUTO_WINDOW_BASE));

    assert!(
        runtime
            .process(index)
            .cow_pages
            .values()
            .any(|page| page.range.start().get() == 0x7000_0000),
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
fn fork_does_not_charge_inherited_pages() {
    let mut runtime = runtime();
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
    let parent = runtime.process(index).ledger.charged();
    assert!(parent >= 0x2000, "能力的常驻页计入父任务");

    context.rax = SYS_FORK;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_ne!(context.rax, u64::MAX);
    let child = runtime.task_count() - 1;
    assert_eq!(
        runtime.process(child).ledger.charged(),
        0,
        "继承页与父任务共享，不得重复计入子任务"
    );
    assert_eq!(runtime.process(index).ledger.charged(), parent);
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
    // fork 后的 Region 才是 COW；显式标记后再触发写时复制。
    context.rax = SYS_REGION_PROTECT;
    context.rdi = 0x4001;
    context.rsi = 0b11 | ((hk_mem::hmp::RegionFlags::COW.bits() as u64) << 32);
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);
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
            .any(|page| page.range.start().get() == 0x7000_1000),
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
