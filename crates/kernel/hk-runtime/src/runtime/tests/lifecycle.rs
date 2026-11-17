//! Provider 回收/迁移/退出与回写通知测试。

use super::*;

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
