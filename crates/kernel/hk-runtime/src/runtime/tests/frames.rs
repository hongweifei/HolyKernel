//! Capacity/Frane 生命周期、句柄复制与退出记账测试。

use super::*;

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
