//! Hosted HolyFS 基准与预算门禁。

use std::time::Instant;

use clap::{Args, ValueEnum};
use hk_block::{MemoryDisk, SECTOR_SIZE};
use hk_fs_holyfs::{DocumentKind, HolyFs, format};

use crate::failure::{Failure, Result};

const IMAGE_BLOCKS: u64 = (64 * 1024 * 1024) / SECTOR_SIZE as u64;
const MIB: f64 = 1024.0 * 1024.0;

/// 基准套件。
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum BenchSuite {
    /// HolyFS 顺序、随机、去重、快照与流式写
    #[value(name = "holyfs")]
    HolyFs,
}

/// 基准参数。
#[derive(Debug, Args)]
pub struct BenchArguments {
    /// 基准套件
    #[arg(long, value_enum, default_value = "holyfs")]
    pub suite: BenchSuite,
    /// 低于预算时返回失败；普通调试构建不建议开启
    #[arg(long)]
    pub enforce: bool,
}

/// 执行基准。
pub fn run(arguments: BenchArguments) -> Result<()> {
    match arguments.suite {
        BenchSuite::HolyFs => holyfs(arguments.enforce),
    }
}

struct Measurement {
    name: &'static str,
    value: f64,
    unit: &'static str,
    budget: Option<f64>,
    higher_is_better: bool,
}

fn holyfs(enforce: bool) -> Result<()> {
    let disk = format(MemoryDisk::new(IMAGE_BLOCKS, SECTOR_SIZE))
        .map_err(|error| Failure::new(format!("格式化 HolyFS 失败：{error:?}")))?;
    let mut fs = HolyFs::mount(disk)
        .map_err(|error| Failure::new(format!("挂载 HolyFS 失败：{error:?}")))?;
    let root = fs.root();
    let document = fs
        .create(root, "bench", DocumentKind::Content)
        .map_err(fs_error)?;

    let data = pseudo_random(1024 * 1024);
    let start = Instant::now();
    fs.write_range(document, 0, &data).map_err(fs_error)?;
    let write = elapsed(start, data.len() as f64);
    fs.sync().map_err(fs_error)?;

    let start = Instant::now();
    let read = fs.read_document_id(document).map_err(fs_error)?;
    let sequential_read = elapsed(start, read.len() as f64);
    if read != data {
        return Err(Failure::new("HolyFS 顺序读写内容不一致"));
    }

    let mut buffer = [0_u8; 4096];
    let start = Instant::now();
    let mut seed = 0x1234_5678_u64;
    for _ in 0..10_000 {
        seed = xorshift(seed);
        let offset = (seed % (data.len() as u64 - 4096)) & !4095;
        fs.read_range(document, offset, &mut buffer)
            .map_err(fs_error)?;
    }
    let random_read = 10_000.0 / start.elapsed().as_secs_f64() / 1000.0;

    let duplicate = fs
        .create(root, "duplicate", DocumentKind::Content)
        .map_err(fs_error)?;
    let start = Instant::now();
    fs.write_range(duplicate, 0, &data).map_err(fs_error)?;
    let dedup = elapsed(start, data.len() as f64);
    if fs.dedup_hits() == 0 {
        return Err(Failure::new("HolyFS 去重未命中"));
    }

    let start = Instant::now();
    let snapshot = fs.snapshot("bench").map_err(fs_error)?;
    let snapshot_create = ms(start);
    let start = Instant::now();
    fs.restore(snapshot).map_err(fs_error)?;
    let snapshot_restore = ms(start);
    fs.forget(snapshot).map_err(fs_error)?;

    let compressed = fs
        .create(root, "compressed", DocumentKind::Content)
        .map_err(fs_error)?;
    let repeated = vec![0x5A_u8; 1024 * 1024];
    let start = Instant::now();
    fs.write_range(compressed, 0, &repeated).map_err(fs_error)?;
    let compression_write = elapsed(start, repeated.len() as f64);
    let start = Instant::now();
    let read = fs.read_document_id(compressed).map_err(fs_error)?;
    let compression_read = elapsed(start, read.len() as f64);
    if read != repeated {
        return Err(Failure::new("HolyFS 压缩内容不一致"));
    }

    let stream = fs
        .create(root, "stream", DocumentKind::Content)
        .map_err(fs_error)?;
    let start = Instant::now();
    let staging = fs.stage(stream, None).map_err(fs_error)?;
    let mut offset = 0_u64;
    for chunk in data.chunks(64 * 1024) {
        fs.write_staging(staging, offset, chunk).map_err(fs_error)?;
        offset += chunk.len() as u64;
    }
    let stream_prepare = ms(start);
    let start = Instant::now();
    let committed = fs.commit_staging(staging, None).map_err(fs_error)?;
    let stream_commit = ms(start);
    if fs.document_info(stream).map_err(fs_error)?.head != committed {
        return Err(Failure::new("HolyFS staging 提交后 head 不一致"));
    }

    let report = [
        Measurement {
            name: "seq write 1 MiB",
            value: write,
            unit: "MiB/s",
            budget: Some(100.0),
            higher_is_better: true,
        },
        Measurement {
            name: "seq read 1 MiB",
            value: sequential_read,
            unit: "MiB/s",
            budget: Some(200.0),
            higher_is_better: true,
        },
        Measurement {
            name: "random 4 KiB read",
            value: random_read,
            unit: "k IOPS",
            budget: Some(5.0),
            higher_is_better: true,
        },
        Measurement {
            name: "dedup write 1 MiB",
            value: dedup,
            unit: "MiB/s",
            budget: None,
            higher_is_better: true,
        },
        Measurement {
            name: "snapshot create",
            value: snapshot_create,
            unit: "ms",
            budget: Some(5.0),
            higher_is_better: false,
        },
        Measurement {
            name: "snapshot restore",
            value: snapshot_restore,
            unit: "ms",
            budget: Some(10.0),
            higher_is_better: false,
        },
        Measurement {
            name: "compress write 1 MiB",
            value: compression_write,
            unit: "MiB/s",
            budget: Some(100.0),
            higher_is_better: true,
        },
        Measurement {
            name: "compress read 1 MiB",
            value: compression_read,
            unit: "MiB/s",
            budget: Some(100.0),
            higher_is_better: true,
        },
        Measurement {
            name: "stream prepare 1 MiB",
            value: stream_prepare,
            unit: "ms",
            budget: Some(100.0),
            higher_is_better: false,
        },
        Measurement {
            name: "stream commit 1 MiB",
            value: stream_commit,
            unit: "ms",
            budget: Some(100.0),
            higher_is_better: false,
        },
    ];

    println!("HolyFS bench：");
    let mut failed = false;
    for item in &report {
        let budget = item.budget.map(|budget| {
            let ok = if item.higher_is_better {
                item.value >= budget
            } else {
                item.value <= budget
            };
            if !ok {
                failed = true;
            }
            format!(
                "{} {budget:.1} {}",
                if item.higher_is_better { "≥" } else { "≤" },
                item.unit
            )
        });
        println!(
            "  {:<22} {:>10.2} {:<7} {}",
            item.name,
            item.value,
            item.unit,
            budget.unwrap_or_default()
        );
    }
    if enforce && failed {
        return Err(Failure::new("HolyFS 基准低于预算"));
    }
    Ok(())
}

fn fs_error(error: hk_fs_holyfs::HolyError) -> Failure {
    Failure::new(format!("HolyFS 基准失败：{error:?}"))
}

fn elapsed(start: Instant, bytes: f64) -> f64 {
    bytes / start.elapsed().as_secs_f64() / MIB
}

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn pseudo_random(length: usize) -> Vec<u8> {
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut data = Vec::with_capacity(length);
    while data.len() < length {
        state = xorshift(state);
        data.extend_from_slice(&state.to_le_bytes());
    }
    data.truncate(length);
    data
}

fn xorshift(state: u64) -> u64 {
    let mut value = state;
    value ^= value << 13;
    value ^= value >> 7;
    value ^= value << 17;
    value
}
