//! Frame 映射、固定与 Region 回滚测试。

use super::*;

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
fn frame_map_failure_after_take_restores_frame() {
    let platform = MockPlatform::new();
    let map_ok = Arc::clone(&platform.map_ok);
    map_ok.store(false, Ordering::Release);
    let mut runtime = runtime_on("b", platform);
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

    context.rax = hk_syscall::SYS_FRAME_MAP;
    context.rdi = frame;
    context.rsi = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX);
    assert!(
        runtime.handle_at(index, frame as usize).is_some(),
        "映射阶段失败必须保留原 Frame 句柄"
    );
    assert_eq!(
        runtime
            .kernel
            .capacity_info(tcb, capacity_handle)
            .expect("Capacity")
            .4,
        hk_hal::PAGE_SIZE as u64,
        "映射阶段失败不得归还物理页"
    );

    map_ok.store(true, Ordering::Release);
    context.rax = hk_syscall::SYS_FRAME_MAP;
    context.rdi = frame;
    context.rsi = 0b11;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_ne!(context.rax, u64::MAX, "恢复后的 Frame 必须可再次映射");
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
