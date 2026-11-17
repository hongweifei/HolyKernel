//! 任务 fork/线程/调度/退出语义测试。

use super::*;

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
    assert!(
        runtime
            .process(index)
            .regions
            .iter()
            .flatten()
            .any(|region| { region.flags.contains(hk_mem::hmp::RegionFlags::COW) }),
        "fork 后父 Region 标记 COW"
    );
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
