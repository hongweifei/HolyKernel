//! Arena 记账与 Capacity Ledger 枚举测试。

use super::*;

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
        before,
        "Arena 分配只保留虚拟区间，不占用物理配额"
    );
    // 首次访问才装入零页并计入 Task Ledger。
    assert!(runtime.anonymous_fault(0, start));
    assert_eq!(
        runtime.process(index).ledger.charged(),
        before + hk_hal::PAGE_SIZE as u64,
        "装入物理页后计入 Task Ledger"
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
fn object_ledger_reports_loader_region_charge() {
    let platform = MockPlatform::new();
    let written = Arc::clone(&platform.written);
    let mut runtime = runtime_on("b", platform);
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let tcb = runtime.tasks[index].tcb;

    // 模拟装载器登记：无 backing 的镜像能力 + 已计费的 Region。
    let capability = hk_mem::hmp::Capability::new(
        hk_mem::hmp::CapabilityKind::TaskImage,
        hk_hal::PAGE_SIZE as u64,
        hk_hal::PAGE_SIZE,
    )
    .expect("能力");
    runtime
        .kernel
        .create_capability(
            tcb,
            capability,
            None,
            hk_handle::Rights::READ | hk_handle::Rights::WRITE | hk_handle::Rights::ADMIN,
        )
        .expect("能力句柄");
    let spec = UserRegionSpec {
        virt: 0x20_0000,
        len: hk_hal::PAGE_SIZE,
        writable: false,
        executable: true,
        source: None,
        provider: None,
    };
    let charge = runtime
        .charge_frame(index, hk_hal::PAGE_SIZE as u64)
        .expect("记账");
    runtime
        .register_region(index, spec, Some(charge))
        .expect("登记 Region");
    let capability_handle = runtime.process(index).regions[0]
        .as_ref()
        .expect("Region")
        .capability;
    let number = runtime
        .install_extra_capability(index, capability_handle)
        .expect("安装能力");

    context.rax = SYS_OBJECT_LEDGER;
    context.rdi = 0x480;
    context.rsi = 48;
    context.rdx = number;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 48);
    let data = written.lock();
    assert_eq!(
        u64::from_le_bytes(data[32..40].try_into().expect("resident")),
        hk_hal::PAGE_SIZE as u64,
        "装载器 Region 的常驻必须可见"
    );
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
