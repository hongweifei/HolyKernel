//! 冒烟结果校验：串口日志、自检音与磁盘镜像。

use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use hk_block::{BlockDevice, DiskSlice, MemoryDisk, SECTOR_SIZE};
use hk_vfs::PathBackend;

use super::{FS_MARKER, SmokeMode};
use crate::disk::HostClock;
use crate::failure::{Failure, Result};

/// 解析 WAV 后端录到的自检音，校验音高与幅度。
pub(super) fn verify_tone(path: &Path) -> Result<()> {
    let bytes = fs::read(path)
        .map_err(|error| Failure::new(format!("读取 {} 失败：{error}", path.display())))?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(Failure::new("音频输出不是 WAV"));
    }
    let mut position = 12;
    let mut format = None;
    let mut data: &[u8] = &[];
    while position + 8 <= bytes.len() {
        let id = &bytes[position..position + 4];
        let size =
            u32::from_le_bytes(bytes[position + 4..position + 8].try_into().unwrap()) as usize;
        let body = position + 8;
        if body + size > bytes.len() {
            break;
        }
        match id {
            b"fmt " => format = Some(&bytes[body..body + size]),
            b"data" => {
                // QEMU 的 wav 后端以流式方式写入：data 长度可能先留 0，
                // PCM 从 data 头之后一直延续到文件末尾。
                data = if size == 0 {
                    &bytes[body..]
                } else {
                    &bytes[body..body + size]
                };
            }
            _ => {}
        }
        position = body + size + (size & 1);
    }
    let Some(format) = format else {
        return Err(Failure::new("WAV 缺少 fmt 块"));
    };
    if format.len() < 16 {
        return Err(Failure::new("WAV fmt 块过短"));
    }
    let channels = u16::from_le_bytes(format[2..4].try_into().unwrap());
    let rate = u32::from_le_bytes(format[4..8].try_into().unwrap());
    let bits = u16::from_le_bytes(format[14..16].try_into().unwrap());
    if bits != 16 || channels == 0 {
        return Err(Failure::new(format!(
            "自检音格式不符：{channels} 声道 / {bits} 位"
        )));
    }
    // 输出后端可能把单声道上混成立体声：按声道跨度取第一个声道。
    let stride = usize::from(channels) * 2;
    let samples: Vec<i16> = data
        .chunks_exact(stride)
        .map(|frame| i16::from_le_bytes([frame[0], frame[1]]))
        .collect();
    let peak = samples
        .iter()
        .map(|sample| sample.unsigned_abs())
        .max()
        .unwrap_or(0);
    if peak < 1_000 {
        return Err(Failure::new(format!("自检音幅度过低：峰值 {peak}")));
    }
    // 服务自检播 440 Hz；客户端在两条流上并发点播 660 Hz 与 880 Hz。
    // 并发叠加后过零率不再等于单音频率，改用 Goertzel 分别测各频率能量，
    // 并以未播放的 550 Hz 作为噪声基准。
    let power = |hz: f64| goertzel(&samples, f64::from(rate), hz);
    let noise = power(550.0).max(1.0);
    for (hz, minimum) in [(440.0, 2), (660.0, 5), (880.0, 8)] {
        let tone = power(hz);
        if tone < noise * 10.0 || tone < minimum as f64 * 1_000.0 {
            return Err(Failure::new(format!(
                "WAV 中 {hz} Hz 能量不足：{tone:.0}（噪声基准 {noise:.0}）"
            )));
        }
    }
    Ok(())
}

/// 单频能量（均方幅度平方），按整段数据归一化。
fn goertzel(samples: &[i16], rate: f64, hz: f64) -> f64 {
    let coefficient = 2.0 * (core::f64::consts::TAU * hz / rate).cos();
    let (mut first, mut second) = (0.0_f64, 0.0_f64);
    for &sample in samples {
        let value = f64::from(sample) + coefficient * first - second;
        second = first;
        first = value;
    }
    let magnitude = first * first + second * second - coefficient * first * second;
    magnitude / samples.len() as f64
}

/// 截取串口日志尾部，避免超时错误淹没输出。
pub(super) fn serial_tail(text: &str) -> String {
    const LIMIT: usize = 8000;
    if text.len() <= LIMIT {
        return text.to_string();
    }
    let mut start = text.len() - LIMIT;
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    format!("…（前文省略）\n{}", &text[start..])
}

/// 等待串口日志出现指定输出。
pub(super) fn wait_for(log: &Path, token: &str, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let text = fs::read(log).unwrap_or_default();
        if String::from_utf8_lossy(&text).contains(token) {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(Failure::new(format!("等待串口输出超时：{token}")));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// 生成新的测试磁盘。
pub(super) fn prepare_disk(mode: SmokeMode, path: &Path) -> Result<PathBuf> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| Failure::new(format!("创建 {} 失败：{error}", parent.display())))?;
    }
    let blocks = crate::disk::DISK_BYTES / SECTOR_SIZE as u64;
    let disk = MemoryDisk::new(blocks, SECTOR_SIZE);
    let disk = if matches!(
        mode,
        SmokeMode::HolyFs
            | SmokeMode::Holydoc
            | SmokeMode::HolyDocResize
            | SmokeMode::Workbench
            | SmokeMode::DisplayRestart
            | SmokeMode::CheckpointServer
    ) {
        crate::disk::seed_holy_docs(disk)?
    } else {
        hk_fs_ext4::format_with_wall_clock(disk, HostClock)
            .map_err(|error| Failure::new(format!("格式化 ext4 测试磁盘失败：{error}")))?
    };
    fs::write(path, disk.into_data())
        .map_err(|error| Failure::new(format!("写入 {} 失败：{error}", path.display())))?;
    Ok(path.to_path_buf())
}

/// 从磁盘镜像重新挂载 HolyFS，确认标记文件已经持久化。
pub(super) fn verify_holyfs(path: &Path) -> Result<()> {
    let bytes = fs::read(path)
        .map_err(|error| Failure::new(format!("读取 {} 失败：{error}", path.display())))?;
    let disk = MemoryDisk::from_data(bytes, SECTOR_SIZE)
        .ok_or_else(|| Failure::new("测试磁盘长度不是扇区整数倍"))?;
    let half = disk.capacity_blocks() / 2;
    let volume = hk_fs_holyfs::HolyFs::mount(DiskSlice::new(disk, 0, half))
        .map_err(|error| Failure::new(format!("重新挂载 HolyFS 失败：{error}")))?;
    let document = volume
        .resolve("/hello")
        .map_err(|error| Failure::new(format!("HolyFS 标记文件缺失：{error:?}")))?;
    let content = volume
        .read_document_id(document)
        .map_err(|error| Failure::new(format!("读取 HolyFS 标记文件失败：{error:?}")))?;
    if content != FS_MARKER {
        return Err(Failure::new(format!("HolyFS 标记内容不符：{:?}", content)));
    }
    Ok(())
}

/// 从磁盘镜像重新挂载 ext4，确认 VFS 写入的标记文件已经持久化。
pub(super) fn verify_ext4(path: &Path) -> Result<()> {
    let bytes = fs::read(path)
        .map_err(|error| Failure::new(format!("读取 {} 失败：{error}", path.display())))?;
    let disk = MemoryDisk::from_data(bytes, SECTOR_SIZE)
        .ok_or_else(|| Failure::new("测试磁盘长度不是扇区整数倍"))?;
    let backend = hk_fs_ext4::Ext4Backend::mount(disk)
        .map_err(|error| Failure::new(format!("重新挂载 ext4 失败：{error}")))?;
    let root = backend.root();
    let node = backend
        .lookup(root, "hello")
        .or_else(|_| {
            backend
                .lookup(root, "disk")
                .and_then(|disk| backend.lookup(disk, "hello"))
        })
        .map_err(|error| Failure::new(format!("ext4 标记文件缺失：{error:?}")))?;
    let mut buffer = [0_u8; 32];
    let read = backend
        .read(node, 0, &mut buffer)
        .map_err(|error| Failure::new(format!("读取 ext4 标记文件失败：{error:?}")))?;
    if &buffer[..read] != FS_MARKER {
        return Err(Failure::new(format!(
            "ext4 标记内容不符：{:?}",
            &buffer[..read]
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[allow(clippy::wildcard_imports)]
    use super::*;

    #[test]
    fn serial_tail_keeps_the_end() {
        let text = "a".repeat(9000) + "tail";
        let tail = serial_tail(&text);
        assert!(tail.ends_with("tail"));
        assert!(tail.len() < text.len());
        assert!(tail.starts_with('…'));
    }

    #[test]
    fn serial_tail_handles_utf8_boundaries() {
        let text = "测".repeat(3000);
        let tail = serial_tail(&text);
        assert!(tail.len() < text.len());
        let body = tail.strip_prefix("…（前文省略）\n").expect("截断标记");
        assert!(body.chars().all(|character| character == '测'));
    }
}
