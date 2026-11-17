//! 懒 TaskImage 缺页装入测试。

use super::*;
use crate::runtime::ImageSource;

fn image_runtime() -> (UserRuntime<MockContext, MockPlatform>, usize) {
    let platform = MockPlatform::new();
    platform.mapped.store(false, Ordering::Release);
    let mut runtime = runtime_on("b", platform);
    let Step::Resume { index, .. } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    (runtime, index)
}

fn register_image(
    runtime: &mut UserRuntime<MockContext, MockPlatform>,
    index: usize,
    len: usize,
    file: &[u8],
    source_phys: u64,
) {
    let tcb = runtime.tasks[index].tcb;
    let capability = hk_mem::hmp::Capability::new(
        hk_mem::hmp::CapabilityKind::TaskImage,
        len as u64,
        hk_hal::PAGE_SIZE,
    )
    .expect("能力");
    runtime
        .kernel
        .create_capability(
            tcb,
            capability,
            None,
            hk_handle::Rights::READ | hk_handle::Rights::EXECUTE | hk_handle::Rights::ADMIN,
        )
        .expect("能力句柄");
    let spec = UserRegionSpec {
        virt: 0x40_0000,
        len,
        writable: false,
        executable: true,
        source: Some(ImageSource {
            phys: hk_hal::PhysAddr::new(source_phys),
            file_len: file.len(),
        }),
    };
    let charge = runtime
        .process_mut(index)
        .ledger
        .charge(len as u64)
        .expect("记账");
    runtime
        .register_region(index, spec, Some(charge))
        .expect("登记镜像 Region");
    let mut memory = runtime.platform.phys.lock();
    for (offset, byte) in file.iter().enumerate() {
        memory.insert(source_phys + offset as u64, *byte);
    }
}

fn resident_bytes(
    runtime: &UserRuntime<MockContext, MockPlatform>,
    index: usize,
    page_index: usize,
) -> Vec<u8> {
    let capability = runtime.process(index).regions[0]
        .as_ref()
        .expect("Region")
        .capability;
    let tcb = runtime.tasks[index].tcb;
    let range = runtime
        .kernel
        .capability_page(tcb, capability, page_index)
        .expect("能力页")
        .expect("已装入页");
    let phys = runtime.platform.phys.lock();
    (0..hk_hal::PAGE_SIZE as u64)
        .map(|offset| phys[&(range.start().get() + offset)])
        .collect()
}

#[test]
fn image_fault_copies_file_and_zero_fills_tail() {
    let (mut runtime, index) = image_runtime();
    register_image(
        &mut runtime,
        index,
        2 * hk_hal::PAGE_SIZE,
        &[0xaa, 0xbb, 0xcc],
        0x7000_0000,
    );

    let outcome = runtime.handle_fault(
        0,
        0x40_0000,
        hk_mem::hmp::provider::ProviderAccess::Read,
        MockContext::default(),
    );
    assert_eq!(outcome, FaultOutcome::Resumed);
    assert_eq!(
        runtime.platform.last_map.load(Ordering::Acquire) as u32,
        (hk_handle::Rights::READ | hk_handle::Rights::EXECUTE).bits()
    );
    let bytes = resident_bytes(&runtime, index, 0);
    assert_eq!(&bytes[..3], &[0xaa, 0xbb, 0xcc]);
    assert!(bytes[3..].iter().all(|byte| *byte == 0));

    let outcome = runtime.handle_fault(
        0,
        0x40_0000 + hk_hal::PAGE_SIZE as u64,
        hk_mem::hmp::provider::ProviderAccess::Read,
        MockContext::default(),
    );
    assert_eq!(outcome, FaultOutcome::Resumed);
    let bytes = resident_bytes(&runtime, index, 1);
    assert!(bytes.iter().all(|byte| *byte == 0), "文件外页必须零填充");
}

#[test]
fn image_fault_mapping_failure_releases_page() {
    let (mut runtime, index) = image_runtime();
    register_image(&mut runtime, index, hk_hal::PAGE_SIZE, &[0xaa], 0x7000_1000);
    let before = runtime.process(index).ledger.charged();
    runtime.platform.map_ok.store(false, Ordering::Release);

    let outcome = runtime.handle_fault(
        0,
        0x40_0000,
        hk_mem::hmp::provider::ProviderAccess::Read,
        MockContext::default(),
    );
    assert_eq!(outcome, FaultOutcome::Unhandled);
    assert_eq!(runtime.process(index).ledger.charged(), before);
    let capability = runtime.process(index).regions[0]
        .as_ref()
        .expect("Region")
        .capability;
    let tcb = runtime.tasks[index].tcb;
    assert!(
        runtime
            .kernel
            .capability_page(tcb, capability, 0)
            .expect("能力页")
            .is_none(),
        "映射失败不能留下能力页"
    );
}

#[test]
fn teardown_removes_image_sources() {
    let (mut runtime, index) = image_runtime();
    register_image(&mut runtime, index, hk_hal::PAGE_SIZE, &[0xaa], 0x7000_2000);
    assert!(!runtime.image_sources.is_empty());
    runtime.teardown_process(index);
    assert!(runtime.image_sources.is_empty());
}
