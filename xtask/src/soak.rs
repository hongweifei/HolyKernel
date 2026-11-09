//! 宿主侧长稳测试：同一 HolyFS 卷上反复创建、读写、校验、删除与快照。

use clap::Args;
use hk_block::{MemoryDisk, SECTOR_SIZE};
use hk_fs_holyfs::{DocumentKind, HolyFs, format};

use crate::failure::{Failure, Result};

const IMAGE_BLOCKS: u64 = (64 * 1024 * 1024) / SECTOR_SIZE as u64;
const DEFAULT_ITERATIONS: u64 = 200;

/// soak 参数。
#[derive(Debug, Args)]
pub struct SoakArguments {
    /// 迭代次数
    #[arg(long, default_value_t = DEFAULT_ITERATIONS)]
    pub iterations: u64,
    /// 随机种子
    #[arg(long, default_value_t = 0x50AC_1234_5678_9ABC)]
    pub seed: u64,
}

/// 执行 soak。
pub fn run(arguments: SoakArguments) -> Result<()> {
    let disk = format(MemoryDisk::new(IMAGE_BLOCKS, SECTOR_SIZE))
        .map_err(|error| Failure::new(format!("格式化 HolyFS 失败：{error:?}")))?;
    let mut fs = HolyFs::mount(disk)
        .map_err(|error| Failure::new(format!("挂载 HolyFS 失败：{error:?}")))?;
    let root = fs.root();
    let mut state = arguments.seed;
    let mut live: Vec<(u64, hk_fs_holyfs::DocId, String)> = Vec::new();

    for iteration in 0..arguments.iterations {
        state = next(state);
        let name = format!("soak-{iteration}");
        let bytes = fill(state, 4096 + (state as usize % 8192));
        let document = fs
            .create(root, &name, DocumentKind::Content)
            .map_err(|error| {
                Failure::new(format!("soak 创建失败 iteration={iteration}: {error:?}"))
            })?;
        fs.write_range(document, 0, &bytes).map_err(|error| {
            Failure::new(format!("soak 写入失败 iteration={iteration}: {error:?}"))
        })?;
        let read = fs.read_document_id(document).map_err(|error| {
            Failure::new(format!("soak 读取失败 iteration={iteration}: {error:?}"))
        })?;
        if read != bytes {
            return Err(Failure::new(format!(
                "soak 内容不一致 iteration={iteration}"
            )));
        }
        live.push((iteration, document, name));

        if iteration % 8 == 7 {
            let (_, document, old_name) = live.remove(0);
            fs.unlink(root, &old_name)
                .or_else(|_| fs.purge(document).map(|()| document))
                .map_err(|error| {
                    Failure::new(format!("soak 删除失败 iteration={iteration}: {error:?}"))
                })?;
        }
        if iteration % 32 == 31 {
            let snapshot = fs.snapshot("soak").map_err(|error| {
                Failure::new(format!("soak 快照失败 iteration={iteration}: {error:?}"))
            })?;
            fs.restore(snapshot).map_err(|error| {
                Failure::new(format!("soak 恢复失败 iteration={iteration}: {error:?}"))
            })?;
            fs.forget(snapshot).map_err(|error| {
                Failure::new(format!(
                    "soak 清理快照失败 iteration={iteration}: {error:?}"
                ))
            })?;
        }
        let report = fs.check().map_err(|error| {
            Failure::new(format!("soak 校验失败 iteration={iteration}: {error:?}"))
        })?;
        if fs.is_damaged() || report.leaked_blocks != 0 {
            return Err(Failure::new(format!(
                "soak 卷状态异常 iteration={iteration}: leaked={}",
                report.leaked_blocks
            )));
        }
        if iteration % 16 == 15 {
            fs.sync().map_err(|error| {
                Failure::new(format!("soak 同步失败 iteration={iteration}: {error:?}"))
            })?;
        }
    }

    fs.scrub()
        .map_err(|error| Failure::new(format!("soak scrub 失败：{error:?}")))?;
    println!("soak：{} 次迭代通过", arguments.iterations);
    Ok(())
}

fn fill(seed: u64, length: usize) -> Vec<u8> {
    let mut state = seed;
    let mut bytes = Vec::with_capacity(length);
    while bytes.len() < length {
        state = next(state);
        bytes.extend_from_slice(&state.to_le_bytes());
    }
    bytes.truncate(length);
    bytes
}

fn next(state: u64) -> u64 {
    let mut value = state;
    value ^= value << 13;
    value ^= value >> 7;
    value ^= value << 17;
    value
}
