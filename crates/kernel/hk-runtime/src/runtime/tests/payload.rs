//! 消息字节负载、Frame 内容读写与消息字查询测试。

use super::*;

#[test]
fn transports_byte_payloads() {
    use spin::Mutex;

    static DATA: Mutex<[u8; 8192]> = Mutex::new([0; 8192]);

    struct DataPlatform;

    impl UserPlatform for DataPlatform {
        fn timer_ticks(&self) -> u64 {
            0
        }

        fn pci_read(&self, _: u8, _: u8, _: u8, _: u8) -> u32 {
            0
        }

        fn zero(&self, _: u64, _: usize) {}

        fn arena_layout(&self, slot: usize) -> (u64, u64) {
            (0x10_0000 + slot as u64 * 0x2_0000, 0x1_0000)
        }

        fn reserve_capacity(
            &self,
            len: usize,
            kind: hk_mem::hmp::CapacityKind,
        ) -> Option<hk_mem::hmp::Capacity> {
            hk_mem::hmp::Capacity::new(
                hk_mem::hmp::CapacityId::next(),
                hk_hal::PhysRange::new(hk_hal::PhysAddr::new(0x9000_0000), len as u64),
                hk_hal::PAGE_SIZE,
                kind,
                0,
            )
            .ok()
        }

        fn user_capacity(&self) -> Option<Arc<spin::Mutex<hk_mem::hmp::Capacity>>> {
            Some(Arc::new(spin::Mutex::new(
                hk_mem::hmp::Capacity::new(
                    hk_mem::hmp::CapacityId::next(),
                    hk_hal::PhysRange::new(hk_hal::PhysAddr::new(0x9000_0000), 0x20_0000),
                    hk_hal::PAGE_SIZE,
                    hk_mem::hmp::CapacityKind::User,
                    0,
                )
                .ok()?,
            )))
        }

        fn frame_bank_stats(&self) -> Option<hk_mem::FrameBankStats> {
            Some(hk_mem::FrameBankStats {
                total: 0x1000,
                used: 0x800,
                peak: 0x900,
                free: 0x800,
                by_capacity: [0x100, 0x200, 0x300, 0, 0],
                by_node: {
                    let mut nodes = [0; hk_syscall::MAX_NUMA_NODES];
                    nodes[0] = 0x800;
                    nodes
                },
            })
        }

        fn read_user(&self, address: u64, buffer: &mut [u8]) -> bool {
            let data = DATA.lock();
            let start = address as usize;
            let Some(end) = start.checked_add(buffer.len()) else {
                return false;
            };
            if end > data.len() {
                return false;
            }
            buffer.copy_from_slice(&data[start..end]);
            true
        }

        fn write_user(&self, address: u64, bytes: &[u8]) -> bool {
            let mut data = DATA.lock();
            let start = address as usize;
            let Some(end) = start.checked_add(bytes.len()) else {
                return false;
            };
            if end > data.len() {
                return false;
            }
            data[start..end].copy_from_slice(bytes);
            true
        }
    }

    let mut runtime = crate::boot::user_runtime(
        &[crate::ServiceSpec::new("a"), crate::ServiceSpec::new("b")],
        DataPlatform,
        |_, endpoint| MockContext {
            rdi: endpoint,
            ..MockContext::default()
        },
    )
    .expect("装配运行时");
    for index in 0..runtime.task_count() {
        runtime.attach(
            index,
            MockContext {
                rdi: index as u64 + 1,
                ..MockContext::default()
            },
        );
    }

    DATA.lock()[0x100..0x105].copy_from_slice(b"hello");
    let Step::Resume {
        index: sender,
        mut context,
    } = runtime.step(0)
    else {
        panic!("发送方应可运行");
    };
    assert_eq!(sender, 0);
    context.rax = SYS_SEND_DATA;
    context.rdi = 2;
    context.rsi = 0x400;
    context.rdx = 0x100;
    context.r10 = 5;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);

    let Step::Resume {
        index: receiver,
        mut context,
    } = runtime.step(0)
    else {
        panic!("接收方应可运行");
    };
    assert_eq!(receiver, 1);
    context.rax = SYS_RECV_DATA;
    context.rdi = 2;
    context.rsi = 0x200;
    context.rdx = 64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0x400);
    assert_eq!(context.r8, 5);
    assert_eq!(&DATA.lock()[0x200..0x205], b"hello");

    runtime.finish(0, receiver);
    DATA.lock()[0x100..0x110].copy_from_slice(b"0123456789abcdef");
    let Step::Resume {
        index: sender,
        mut context,
    } = runtime.step(0)
    else {
        panic!("发送方应可再次运行");
    };
    assert_eq!(sender, 0);
    context.rax = SYS_SEND_DATA;
    context.rdi = 2;
    context.rsi = 0x400;
    context.rdx = 0x100;
    context.r10 = 16;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    runtime.finish(0, sender);

    let Step::Resume {
        index: receiver,
        mut context,
    } = runtime.step(0)
    else {
        panic!("接收方应可再次运行");
    };
    assert_eq!(receiver, 1);
    context.rax = SYS_RECV_DATA;
    context.rdi = 2;
    context.rsi = 0x200;
    context.rdx = 8;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0x400);
    assert_eq!(context.r8, 8);
    assert_eq!(&DATA.lock()[0x200..0x208], b"01234567");

    context.rax = hk_syscall::SYS_ARENA_INFO;
    context.rdi = 0x300;
    context.rsi = 32;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 32);
    let data = DATA.lock();
    let base = u64::from_le_bytes(data[0x300..0x308].try_into().expect("base"));
    let size = u64::from_le_bytes(data[0x308..0x310].try_into().expect("size"));
    let regions = u64::from_le_bytes(data[0x318..0x320].try_into().expect("regions"));
    assert_ne!(base, 0, "Arena 应报告真实基址");
    assert!(size > 0);
    assert_eq!(regions, 0, "Arena 分配前不应有 Region");
    drop(data);

    context.rax = hk_syscall::SYS_FRAME_BANK_STATS;
    context.rdi = 0x400;
    let stats_bytes = 32 + hk_syscall::CAPACITY_KIND_COUNT * 8 + hk_syscall::MAX_NUMA_NODES * 8;
    context.rsi = stats_bytes as u64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, stats_bytes as u64);
    let data = DATA.lock();
    assert_eq!(
        u64::from_le_bytes(data[0x400..0x408].try_into().expect("total")),
        0x1000
    );
    assert_eq!(
        u64::from_le_bytes(data[0x408..0x410].try_into().expect("used")),
        0x800
    );
    assert_eq!(
        u64::from_le_bytes(data[0x410..0x418].try_into().expect("peak")),
        0x900
    );
    assert_eq!(
        u64::from_le_bytes(data[0x418..0x420].try_into().expect("free")),
        0x800
    );
    assert_eq!(
        u64::from_le_bytes(data[0x420..0x428].try_into().expect("system")),
        0x100
    );
}

#[test]
fn frame_content_roundtrips_through_user_buffers() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let tcb = runtime.tasks[index].tcb;
    let capacity = hk_mem::hmp::Capacity::new(
        hk_mem::hmp::CapacityId::new(98),
        hk_hal::PhysRange::new(hk_hal::PhysAddr::new(0x60_0000), 0x1_0000),
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
    let capacity_number = 0x5002;

    context.rax = hk_syscall::SYS_CAPACITY_ALLOC;
    context.rdi = capacity_number;
    context.rsi = hk_hal::PAGE_SIZE as u64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    let frame = context.rax;
    assert_ne!(frame, u64::MAX);

    let content = b"revision-page";
    runtime.platform.input.lock().extend_from_slice(content);
    context.rax = hk_syscall::SYS_FRAME_WRITE;
    context.rdi = frame;
    context.rsi = 0;
    context.rdx = 0;
    context.r10 = content.len() as u64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0, "Provider 必须能写入 Frame 内容");
    assert_eq!(
        runtime.platform.phys.lock().len(),
        content.len(),
        "物理页必须收到写入内容"
    );

    runtime.platform.written.lock().clear();
    context.rax = hk_syscall::SYS_FRAME_READ;
    context.rdi = frame;
    context.rsi = 0;
    context.rdx = 0;
    context.r10 = content.len() as u64;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 0);
    assert_eq!(runtime.platform.written.lock().as_slice(), content);
}

#[test]
fn message_words_are_queryable() {
    let mut runtime = runtime();
    let Step::Resume { index, mut context } = runtime.step(0) else {
        panic!("任务应可运行");
    };
    let words = [1_u64, 2, 3, 4, 5, 6, 7, 8];
    runtime.tasks[index].last_words = words;

    context.rax = hk_syscall::SYS_MESSAGE_WORDS;
    context.rdi = 0x100;
    assert_eq!(runtime.dispatch(0, &mut context), Switch::Continue);
    assert_eq!(context.rax, 64);
    let written = runtime.platform.written.lock();
    for (slot, word) in words.iter().enumerate() {
        assert_eq!(
            u64::from_le_bytes(written[slot * 8..slot * 8 + 8].try_into().expect("word")),
            *word
        );
    }
}
