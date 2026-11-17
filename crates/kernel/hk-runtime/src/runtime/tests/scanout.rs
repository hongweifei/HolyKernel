//! Scanout 驻留与固定语义测试。

use super::*;

#[test]
fn scanout_requires_surface_and_holds_pin() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = SYS_CAPABILITY_CREATE;
    context.rdi = 1;
    context.rsi = 0x1000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let surface = context.rax;
    assert_ne!(surface, u64::MAX);

    context.rax = SYS_CAPABILITY_SCANOUT;
    context.rdi = surface;
    context.rsi = 1;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0, "Surface 可以进入 scanout 驻留");

    context.rax = SYS_CAPABILITY_PIN;
    context.rdi = surface;
    context.rsi = 0;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX, "scanout 期间不能解除固定");

    context.rax = SYS_CAPABILITY_SCANOUT;
    context.rdi = surface;
    context.rsi = 0;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);

    context.rax = SYS_CAPABILITY_PIN;
    context.rdi = surface;
    context.rsi = 0;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0, "退出 scanout 后可解除固定");

    context.rax = SYS_CAPABILITY_CREATE_LAZY;
    context.rdi = 4;
    context.rsi = 0x1000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let anonymous = context.rax;
    assert_ne!(anonymous, u64::MAX);
    context.rax = SYS_CAPABILITY_SCANOUT;
    context.rdi = anonymous;
    context.rsi = 1;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX, "匿名能力不能 scanout");

    let _ = runtime.finish(0, index);
}

#[test]
fn closing_region_during_scanout_is_rejected() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };

    context.rax = SYS_CAPABILITY_CREATE;
    context.rdi = 1;
    context.rsi = 0x1000;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let surface = context.rax;
    assert_ne!(surface, u64::MAX);

    context.rax = SYS_CAPABILITY_SCANOUT;
    context.rdi = surface;
    context.rsi = 1;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);

    context.rax = SYS_REGION_MAP;
    context.rdi = surface;
    context.rsi = 0;
    context.rdx = 0x1000;
    context.r10 = 0b11 | ((hk_mem::hmp::RegionFlags::PINNED.bits() as u64) << 32);
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let region = context.rax;
    assert_ne!(region, u64::MAX);

    context.rax = SYS_HANDLE_CLOSE;
    context.rdi = region;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, u64::MAX, "scanout 驻留期间不能关闭 Region");
    assert_eq!(
        runtime.process(0).regions.iter().flatten().count(),
        1,
        "拒绝关闭时 Region 与映射必须保留"
    );

    context.rax = SYS_CAPABILITY_SCANOUT;
    context.rdi = surface;
    context.rsi = 0;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);

    context.rax = SYS_HANDLE_CLOSE;
    context.rdi = region;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0, "退出 scanout 后可关闭 Region");
    let _ = runtime.finish(0, index);
}
