//! 文件系统自检：VFS 读写、ext4 标记、驱动重启与读基准。

use hk_proto::block::BLOCK_SHUTDOWN;
use hk_proto::vfs::{OPEN_CREATE, OPEN_READ, OPEN_WRITE};
use hk_proto_client::VfsClient;
use hk_user_api::{send, service_endpoint, service_restarts, tsc, tsc_frequency, write, yield_now};

/// 验证 VFS 读写、截断、删除与内建文件系统。
pub(super) fn verify_vfs(endpoint: u64) -> bool {
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
fn ext4_paths(client: &mut VfsClient) -> (&'static str, &'static str) {
    if client.stat("/disk").is_ok() {
        ("/disk/hello", "/disk/bench")
    } else {
        ("/hello", "/bench")
    }
}

/// 经 VFS 写入并读回 ext4 标记文件。
pub(super) fn verify_ext4(endpoint: u64) -> bool {
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
pub(super) fn verify_driver_restart(endpoint: u64) -> bool {
    let old = service_endpoint("hk-server-virtio");
    if old == 0 {
        return false;
    }
    let before = service_restarts("hk-server-virtio");
    send(old, BLOCK_SHUTDOWN, 0);
    let mut restarted = false;
    // 重启复用同一任务槽位，端点编号不变；用累计重启次数判定。
    for _ in 0..100_000 {
        if service_restarts("hk-server-virtio") > before {
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
pub(super) fn benchmark_ext4(endpoint: u64) -> Option<u64> {
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
            .write_buffer(handle, (index * CHUNK) as u64, &chunk)
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
        let length = client.read_buffer(handle, total as u64, CHUNK).ok()?;
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
