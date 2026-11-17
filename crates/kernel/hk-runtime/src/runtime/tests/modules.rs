//! 启动模块读取 ABI 测试。

use super::*;
use hk_syscall::{SYS_MODULE_COUNT, SYS_MODULE_INFO, SYS_MODULE_READ};

fn image_runtime() -> UserRuntime<MockContext, MockPlatform> {
    let platform = MockPlatform::new();
    for (offset, byte) in [0x11, 0x22, 0x33, 0x44, 0x55].iter().enumerate() {
        platform
            .phys
            .lock()
            .insert(0x7200_0000 + offset as u64, *byte);
    }
    let mut runtime = runtime_on("hk-server-image", platform);
    runtime.register_modules(alloc::vec![ModuleRecord {
        name: String::from("hk-client-image-test"),
        range: hk_hal::PhysRange::new(hk_hal::PhysAddr::new(0x7200_0000), 5),
    }]);
    runtime.current[0] = Some(1);
    runtime
}

#[test]
fn image_server_reads_module() {
    let mut runtime = image_runtime();
    let mut context = MockContext {
        rax: SYS_MODULE_COUNT,
        ..Default::default()
    };
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 1);

    context.rax = SYS_MODULE_INFO;
    context.rdi = 0;
    context.rsi = 0x500;
    context.rdx = 64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let written = context.rax as usize;
    assert_eq!(
        written,
        hk_syscall::MODULE_INFO_BYTES + "hk-client-image-test".len()
    );
    let data = runtime.platform.written.lock();
    assert_eq!(u64::from_le_bytes(data[..8].try_into().expect("size")), 5);
    assert_eq!(&data[16..written], b"hk-client-image-test");
    drop(data);

    context.rax = SYS_MODULE_READ;
    context.rdi = 0;
    context.rsi = 2;
    context.rdx = 0x600;
    context.r10 = 3;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 3);
    assert_eq!(&runtime.platform.written.lock()[..3], &[0x33, 0x44, 0x55]);
}

#[test]
fn module_read_is_limited_to_image_server() {
    let platform = MockPlatform::new();
    let mut runtime = runtime_on("b", platform);
    runtime.register_modules(alloc::vec![ModuleRecord {
        name: String::from("a"),
        range: hk_hal::PhysRange::new(hk_hal::PhysAddr::new(0x7200_0000), 1),
    }]);
    runtime.current[0] = Some(0);
    let mut context = MockContext {
        rax: SYS_MODULE_COUNT,
        ..Default::default()
    };
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX);

    context.rax = SYS_MODULE_READ;
    context.rdi = 0;
    context.rsi = 0;
    context.rdx = 0x600;
    context.r10 = 1;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX);
}
