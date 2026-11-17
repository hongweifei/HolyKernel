//! Region 标志、能力权限、Capacity 合并与 Arena 记账测试。

use super::*;

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
    assert_eq!(context.rax, u64::MAX, "只有 Anonymous 能力可以带 GROW 映射");

    context.rax = SYS_REGION_MAP;
    context.r10 = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_ne!(context.rax, u64::MAX, "去掉了 GROW 的非匿名映射仍应允许");
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
    context.rdx = 72;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 72);

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
    context.rdx = 72;
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
            unmap_ok: Arc::new(AtomicBool::new(false)),
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
