//! 演示服务：向 echo 服务发送请求，等待回复后验证并退出。
//!
//! 本 crate 只在 `x86_64-unknown-none` 目标上构建；宿主构建保留占位入口，便于
//! `cargo clippy --workspace --all-targets` 与工作区测试。

#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
extern crate alloc;

#[cfg(target_os = "none")]
use hk_server_api::heap::FreeHeap;

/// 服务堆大小；协程栈从该堆分配。
#[cfg(target_os = "none")]
const HEAP_SIZE: usize = 256 * 1024;

#[cfg(target_os = "none")]
#[global_allocator]
static ALLOCATOR: FreeHeap = FreeHeap::new(HEAP_SIZE);

#[cfg(target_os = "none")]
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
#[cfg(target_os = "none")]
use hk_coroutine::{Coroutine, State, suspend};
#[cfg(target_os = "none")]
use hk_server_api::block::BLOCK_SHUTDOWN;
#[cfg(target_os = "none")]
use hk_server_api::vfs::{OPEN_CREATE, OPEN_READ, OPEN_WRITE, VfsClient};

#[cfg(target_os = "none")]
use hk_server_api::{
    ENDPOINT_ECHO, alloc, boot_cmdline, call, exit, futex_wait, futex_wake, recv, send,
    send_handle, service_endpoint, thread_create, thread_join, tsc, tsc_frequency, wait_notify,
    write, write_u64, yield_now,
};

/// 协程共享的执行轨迹。
#[cfg(target_os = "none")]
static COROUTINE_TRACE: AtomicU32 = AtomicU32::new(0);

/// 线程共享的累加计数。
#[cfg(target_os = "none")]
static THREAD_COUNT: AtomicU64 = AtomicU64::new(0);

/// 线程共享的唤醒字。
#[cfg(target_os = "none")]
static THREAD_WORD: AtomicU32 = AtomicU32::new(0);

/// 请求标签。
#[cfg(target_os = "none")]
const REQUEST: u64 = 0x100;

/// 回复标签。
#[cfg(target_os = "none")]
const REPLY: u64 = 0x101;

/// 句柄转移标签。
#[cfg(target_os = "none")]
const HANDOFF: u64 = 0x200;

/// 句柄转移回复标签。
#[cfg(target_os = "none")]
const HANDOFF_REPLY: u64 = 0x201;

/// 通知请求标签。
#[cfg(target_os = "none")]
const NOTIFY_REQUEST: u64 = 0x300;

/// 服务入口：内核以用户态进入，参数是内核分配给本服务的端点编号。
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start(endpoint: u64) -> ! {
    write(b"hk-server-demo running\n");
    let (label, word, _sender) = call(ENDPOINT_ECHO, REQUEST, 21);

    if label == REPLY && word == 42 {
        write("hk-server-demo 收到回复 42\n".as_bytes());
        if verify_arena() {
            write(b"hk-server-demo arena ok\n");
        } else {
            write("hk-server-demo arena 失败\n".as_bytes());
            exit(1)
        }
        if verify_threads() {
            write(b"hk-server-demo thread ok\n");
        } else {
            write(b"hk-server-demo thread fail\n");
            exit(1)
        }
        if verify_coroutines() {
            write(b"hk-server-demo coroutine ok\n");
        } else {
            write(b"hk-server-demo coroutine fail\n");
            exit(1)
        }

        send(ENDPOINT_ECHO, NOTIFY_REQUEST, 0);
        if wait_notify(endpoint, 1) != 1 {
            write("hk-server-demo 通知等待失败\n".as_bytes());
            exit(1)
        }
        write("hk-server-demo 通知等待 ok\n".as_bytes());

        send_handle(ENDPOINT_ECHO, HANDOFF, 7, ENDPOINT_ECHO);
        let (label, word, _sender) = recv(endpoint);
        if label != HANDOFF_REPLY || word != 14 {
            write("hk-server-demo 句柄转移失败\n".as_bytes());
            exit(1)
        }
        write("hk-server-demo 句柄转移 ok\n".as_bytes());
        if verify_vfs(endpoint) {
            write("VFS 读写 ok\n".as_bytes());
        } else {
            write("VFS 读写失败\n".as_bytes());
            exit(1)
        }
        if verify_ext4(endpoint) {
            write("hk-server-demo fs ok\n".as_bytes());
        } else {
            write("hk-server-demo fs 失败\n".as_bytes());
            exit(1)
        }

        let mut cmdline_buffer = [0_u8; 128];
        let cmdline = boot_cmdline(&mut cmdline_buffer).unwrap_or("");
        if cmdline.contains("restart-test") {
            if verify_driver_restart(endpoint) {
                write("hk-server-demo 驱动重启 ok\n".as_bytes());
            } else {
                write("hk-server-demo 驱动重启失败\n".as_bytes());
                exit(1)
            }
        }
        // 没有块设备时不跑 ext4 顺序读基准，避免 tmpfs 上长时间阻塞启动。
        if service_endpoint("hk-server-virtio") != 0
            && let Some(tenths) = benchmark_ext4(endpoint)
        {
            write("fs 1 MiB 读：".as_bytes());
            write_u64(tenths / 10);
            write(".".as_bytes());
            write_u64(tenths % 10);
            write(" MiB/s\n".as_bytes());
        }
        exit(0)
    }

    write("hk-server-demo 回复异常\n".as_bytes());
    exit(1)
}

#[cfg(target_os = "none")]
fn verify_vfs(endpoint: u64) -> bool {
    let mut client = VfsClient::new("hk-server-path", endpoint);
    let Ok(handle) = client.open("/tmp/hello", OPEN_READ | OPEN_WRITE | OPEN_CREATE) else {
        return false;
    };
    if client.write(handle, 0, b"vfs").is_err() {
        return false;
    }
    let mut buffer = [0_u8; 16];
    let Ok(length) = client.read(handle, 0, &mut buffer) else {
        return false;
    };
    if length < 3 || &buffer[..3] != b"vfs" {
        return false;
    }
    if client.close(handle).is_err() {
        return false;
    }
    if client.stat("/tmp/hello").ok() != Some((1, 3)) {
        return false;
    }
    if client.truncate("/tmp/hello", 1).is_err() {
        return false;
    }
    if client.stat("/tmp/hello").ok() != Some((1, 1)) {
        return false;
    }
    if client.unlink("/tmp/hello").is_err() {
        return false;
    }
    if client.stat("/tmp/hello").is_ok() {
        return false;
    }

    let Ok(handle) = client.open("/proc/version", OPEN_READ) else {
        return false;
    };
    let mut version = [0_u8; 32];
    let Ok(length) = client.read(handle, 0, &mut version) else {
        return false;
    };
    if !version[..length].starts_with(b"HolyKernel") {
        return false;
    }
    let _ = client.close(handle);

    let Ok(handle) = client.open("/dev/null", OPEN_WRITE) else {
        return false;
    };
    if client.write(handle, 0, b"drop") != Ok(4) {
        return false;
    }
    let _ = client.close(handle);
    if client.stat("/dev/null").ok() != Some((1, 0)) {
        return false;
    }

    let Ok(handle) = client.open("/dev/zero", OPEN_READ) else {
        return false;
    };
    let mut zeros = [0xFF_u8; 4];
    let read = client.read(handle, 0, &mut zeros);
    if read != Ok(4) {
        return false;
    }
    if zeros != [0; 4] {
        return false;
    }
    let _ = client.close(handle);

    let mut mounts = [0_u8; 128];
    let Ok(length) = client.mounts(&mut mounts) else {
        return false;
    };
    let mounts = &mounts[..length];
    ["/tmp", "/dev", "/proc"].iter().all(|target| {
        mounts
            .split(|byte| *byte == b'\n')
            .any(|line| line == target.as_bytes())
    })
}
/// 返回 ext4 标记与基准路径：数据盘挂载用 `/disk`，主根挂载用 `/`。
#[cfg(target_os = "none")]
fn ext4_paths(client: &mut VfsClient) -> (&'static str, &'static str) {
    if client.stat("/disk").is_ok() {
        ("/disk/hello", "/disk/bench")
    } else {
        ("/hello", "/bench")
    }
}

/// 经 VFS 写入并读回 ext4 标记文件。
#[cfg(target_os = "none")]
fn verify_ext4(endpoint: u64) -> bool {
    const CONTENT: &[u8] = b"HolyKernel ext4 ok\n";
    let mut client = VfsClient::new("hk-server-path", endpoint);
    let (marker, _) = ext4_paths(&mut client);
    let Ok(handle) = client.open(marker, OPEN_READ | OPEN_WRITE | OPEN_CREATE) else {
        return false;
    };
    if client.write(handle, 0, CONTENT).is_err() {
        return false;
    }
    let _ = client.close(handle);
    if client.sync().is_err() {
        return false;
    }

    let Ok(handle) = client.open(marker, OPEN_READ) else {
        return false;
    };
    let mut buffer = [0_u8; 32];
    let Ok(length) = client.read(handle, 0, &mut buffer) else {
        return false;
    };
    let _ = client.close(handle);
    length == CONTENT.len() && buffer[..length] == *CONTENT
}

/// 请求块驱动退出并确认监督重启后 ext4 仍可读。
#[cfg(target_os = "none")]
fn verify_driver_restart(endpoint: u64) -> bool {
    let old = service_endpoint("hk-server-virtio");
    if old == 0 {
        return false;
    }
    send(old, BLOCK_SHUTDOWN, 0);
    let mut restarted = false;
    for _ in 0..100_000 {
        let current = service_endpoint("hk-server-virtio");
        if current != 0 && current != old {
            restarted = true;
            break;
        }
        let _ = yield_now();
    }
    if !restarted {
        return false;
    }

    let mut client = VfsClient::new("hk-server-path", endpoint);
    let (marker, _) = ext4_paths(&mut client);
    for _ in 0..1_000 {
        if let Ok(handle) = client.open(marker, OPEN_READ) {
            let mut buffer = [0_u8; 32];
            if let Ok(length) = client.read(handle, 0, &mut buffer) {
                let _ = client.close(handle);
                if buffer[..length] == *b"HolyKernel ext4 ok\n" {
                    write("fs 重连 ok\n".as_bytes());
                    return true;
                }
            }
        }
        let _ = yield_now();
    }
    false
}

/// 写入并顺序读取 1 MiB，返回 0.1 MiB/s 单位。
#[cfg(target_os = "none")]
fn benchmark_ext4(endpoint: u64) -> Option<u64> {
    const CHUNK: usize = 60 * 1024;
    const CHUNKS: usize = 17;
    let mut client = VfsClient::new("hk-server-path", endpoint);
    let (_, path) = ext4_paths(&mut client);
    let handle = client
        .open(path, OPEN_READ | OPEN_WRITE | OPEN_CREATE)
        .ok()?;
    let chunk = [0x5A_u8; CHUNK];
    for index in 0..CHUNKS {
        client
            .write_frame(handle, (index * CHUNK) as u64, &chunk)
            .ok()?;
    }
    let _ = client.close(handle);
    client.sync().ok()?;

    let handle = client.open(path, OPEN_READ).ok()?;
    let frequency = tsc_frequency();
    if frequency == 0 {
        return None;
    }
    let start = tsc();
    let mut total = 0;
    loop {
        let length = client.read_frame(handle, total as u64, CHUNK).ok()?;
        if length == 0 {
            break;
        }
        total += length;
    }
    let elapsed = tsc().saturating_sub(start);
    let _ = client.close(handle);
    if total != CHUNK * CHUNKS || elapsed == 0 {
        return None;
    }
    let bytes = total as u128 * u128::from(frequency) / u128::from(elapsed);
    Some((bytes * 10 / (1024 * 1024)) as u64)
}

#[cfg(target_os = "none")]
fn verify_arena() -> bool {
    let pointer = alloc(4096, 16);
    if pointer.is_null() {
        return false;
    }

    // SAFETY: 内核在任务 Arena 内分配了 4096 字节，地址有效且可写。
    let buffer = unsafe { core::slice::from_raw_parts_mut(pointer, 4096) };
    buffer.fill(0xA5);
    buffer.iter().all(|&byte| byte == 0xA5)
}

/// 线程入口：校验 TLS、累加共享计数并唤醒等待者。
#[cfg(target_os = "none")]
extern "C" fn thread_worker(argument: u64) -> ! {
    let tls: u64;
    // SAFETY: 运行库在切到本线程前把 FS 基址设为 `argument`。
    unsafe {
        core::arch::asm!("mov {}, fs:[0]", out(reg) tls, options(nostack, preserves_flags));
    }
    if tls != argument {
        write(b"hk-server-demo thread tls fail\n");
        exit(1)
    }
    for _ in 0..1000 {
        THREAD_COUNT.fetch_add(1, Ordering::Relaxed);
    }
    THREAD_WORD.store(1, Ordering::Release);
    futex_wake(core::ptr::addr_of!(THREAD_WORD) as u64, 1);
    exit(0)
}

/// 验证同地址空间线程、TLS 与共享内存等待。
#[cfg(target_os = "none")]
fn verify_threads() -> bool {
    const STACK: usize = 64 * 1024;
    let stack = alloc(STACK, 16);
    let tls = alloc(16, 8);
    if stack.is_null() || tls.is_null() {
        return false;
    }
    // SAFETY: `tls` 是 Arena 内已清零的 8 字节对齐区间。
    unsafe { (tls as *mut u64).write(tls as u64) };
    THREAD_COUNT.store(0, Ordering::Relaxed);
    THREAD_WORD.store(0, Ordering::Relaxed);
    let thread = thread_create(
        thread_worker as *const () as u64,
        stack as u64 + STACK as u64 - 8,
        tls as u64,
        tls as u64,
    );
    if thread == u64::MAX {
        return false;
    }
    let word = core::ptr::addr_of!(THREAD_WORD) as u64;
    let _ = futex_wait(word, 0);
    if thread_join(thread) != 0 {
        return false;
    }
    THREAD_COUNT.load(Ordering::Relaxed) == 1000 && THREAD_WORD.load(Ordering::Relaxed) == 1
}

/// 验证两个协程在同一线程内交替执行。
#[cfg(target_os = "none")]
fn verify_coroutines() -> bool {
    COROUTINE_TRACE.store(0, Ordering::Relaxed);
    let Some(mut first) = Coroutine::new(16 * 1024, || {
        COROUTINE_TRACE.fetch_add(1, Ordering::Relaxed);
        suspend();
        COROUTINE_TRACE.fetch_add(100, Ordering::Relaxed);
    }) else {
        return false;
    };
    let Some(mut second) = Coroutine::new(16 * 1024, || {
        COROUTINE_TRACE.fetch_add(10, Ordering::Relaxed);
        suspend();
        COROUTINE_TRACE.fetch_add(1000, Ordering::Relaxed);
    }) else {
        return false;
    };

    if first.resume() != State::Suspended || second.resume() != State::Suspended {
        return false;
    }
    if COROUTINE_TRACE.load(Ordering::Relaxed) != 11 {
        return false;
    }
    if first.resume() != State::Finished || second.resume() != State::Finished {
        return false;
    }
    COROUTINE_TRACE.load(Ordering::Relaxed) == 1111
}

hk_server_api::server_boilerplate!();
