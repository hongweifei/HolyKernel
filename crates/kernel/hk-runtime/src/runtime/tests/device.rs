//! DMA 映射、设备授权与 IRQ/失败唤醒测试。

use super::*;

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
fn failed_service_wakes_inflight_recv_with_error() {
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
    // 只等待该实例消息的接收者不会再有输入：返回错误而不是空消息。
    assert_eq!(runtime.tasks[index].context.rax, u64::MAX);
    assert_eq!(runtime.tasks[index].context.r8, 0);
}
