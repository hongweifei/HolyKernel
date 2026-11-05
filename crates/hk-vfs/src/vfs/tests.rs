//! VFS 单元测试。

use super::*;

use crate::Tmpfs;

#[test]
fn creates_writes_and_reads_files() {
    let mut vfs = Vfs::new_path(Tmpfs::new());
    vfs.create_dir("/tmp").expect("创建目录");
    let handle = vfs.open("/tmp/hello", OpenFlags::CREATE).expect("打开文件");
    assert_eq!(vfs.write(handle, b"hello").expect("写入"), 5);
    vfs.close(handle).expect("关闭");

    let handle = vfs
        .open("/tmp/hello", OpenFlags::READ_ONLY)
        .expect("重新打开");
    let mut buffer = [0_u8; 16];
    let read = vfs.read(handle, &mut buffer).expect("读取");
    assert_eq!(&buffer[..read], b"hello");
    assert_eq!(vfs.metadata("/tmp/hello").expect("元数据").size, 5);
    assert_eq!(vfs.readdir("/tmp").expect("目录").len(), 1);
}

#[test]
fn mounts_cross_path_boundaries() {
    let mut vfs = Vfs::new_path(Tmpfs::new());
    vfs.create_dir("/mnt").expect("创建挂载点");
    vfs.mount_path("/mnt", Tmpfs::new()).expect("挂载");

    let handle = vfs.open("/mnt/file", OpenFlags::CREATE).expect("打开");
    vfs.write(handle, b"mounted").expect("写入");
    vfs.close(handle).expect("关闭");

    let handle = vfs.open("/mnt/file", OpenFlags::READ_ONLY).expect("打开");
    let mut buffer = [0_u8; 8];
    let read = vfs.read(handle, &mut buffer).expect("读取");
    assert_eq!(&buffer[..read], b"mounted");
    assert!(matches!(
        vfs.open("/mnt", OpenFlags::WRITE_ONLY),
        Err(VfsError::IsDirectory)
    ));
    vfs.umount("/mnt").expect("卸载");
    assert!(matches!(
        vfs.open("/mnt/file", OpenFlags::READ_ONLY),
        Err(VfsError::NotFound)
    ));
}

#[test]
fn rejects_invalid_parents() {
    let mut vfs = Vfs::new_path(Tmpfs::new());
    assert!(matches!(
        vfs.open("/missing/file", OpenFlags::CREATE),
        Err(VfsError::NotFound)
    ));
    let handle = vfs.open("/file", OpenFlags::CREATE).expect("创建文件");
    vfs.close(handle).expect("关闭");
    assert!(matches!(
        vfs.open("/file/child", OpenFlags::CREATE),
        Err(VfsError::NotDirectory)
    ));
}
#[test]
fn truncates_unlinks_and_reads_at_offsets() {
    let mut vfs = Vfs::new_path(Tmpfs::new());
    let handle = vfs.open("/file", OpenFlags::CREATE).expect("创建");
    vfs.write(handle, b"hello").expect("写入");
    let mut buffer = [0_u8; 8];
    assert_eq!(vfs.read_at(handle, 1, &mut buffer).expect("偏移读"), 4);
    assert_eq!(&buffer[..4], b"ello");
    vfs.truncate("/file", 2).expect("截断");
    assert_eq!(vfs.metadata("/file").expect("元数据").size, 2);
    vfs.close(handle).expect("关闭");
    vfs.unlink("/file").expect("删除");
    assert!(matches!(vfs.metadata("/file"), Err(VfsError::NotFound)));
}

#[test]
fn device_backends_bypass_page_cache() {
    let mut vfs = Vfs::new_path(Tmpfs::new());
    vfs.mount_path("/dev", crate::Devfs::new())
        .expect("挂载 devfs");
    let handle = vfs
        .open("/dev/zero", OpenFlags::READ_ONLY)
        .expect("打开 zero");
    let mut buffer = [0xFF_u8; 4];
    assert_eq!(vfs.read(handle, &mut buffer).expect("读取"), 4);
    assert_eq!(buffer, [0; 4]);
    vfs.close(handle).expect("关闭");
}

#[test]
fn sequential_read_1mib() {
    let mut vfs = Vfs::new_path(Tmpfs::new());
    let handle = vfs.open("/large", OpenFlags::CREATE).expect("创建");
    let chunk = [0x5A_u8; 4096];
    for _ in 0..256 {
        vfs.write(handle, &chunk).expect("写入");
    }
    vfs.close(handle).expect("关闭");

    let handle = vfs.open("/large", OpenFlags::READ_ONLY).expect("打开");
    let start = std::time::Instant::now();
    let mut buffer = [0_u8; 4096];
    let mut total = 0;
    loop {
        let read = vfs.read(handle, &mut buffer).expect("读取");
        if read == 0 {
            break;
        }
        total += read;
    }
    let elapsed = start.elapsed();
    assert_eq!(total, 1024 * 1024);
    std::eprintln!(
        "VFS 顺序读：{} MiB/s",
        (total as f64 / (1024.0 * 1024.0)) / elapsed.as_secs_f64()
    );
}
