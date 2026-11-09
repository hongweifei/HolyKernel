//! 宿主侧确定性 fuzz：解析器随机输入与 HolyFS 随机卷。

use std::panic::{AssertUnwindSafe, catch_unwind};

use clap::Args;
use hk_block::{MemoryDisk, SECTOR_SIZE};
use hk_fs_holyfs::HolyFs;

use crate::failure::{Failure, Result};

/// 默认迭代次数。
const DEFAULT_ITERATIONS: u64 = 10_000;

/// fuzz 参数。
#[derive(Debug, Args)]
pub struct FuzzArguments {
    /// 迭代次数
    #[arg(long, default_value_t = DEFAULT_ITERATIONS)]
    pub iterations: u64,
    /// 随机种子
    #[arg(long, default_value_t = 0x5EED_1234_5678_9ABC)]
    pub seed: u64,
}

/// 执行确定性 fuzz。
pub fn run(arguments: FuzzArguments) -> Result<()> {
    let mut state = arguments.seed;
    for iteration in 0..arguments.iterations {
        state = next(state);
        let length = (state as usize) % 8192;
        let bytes = random_bytes(state, length);
        run_case(iteration, arguments.seed, || {
            let _ = hk_fw::firmware::parse_fadt(&bytes);
            let _ = hk_fw::firmware::parse_mcfg(&bytes);
            let _ = hk_fw::firmware::parse_srat(&bytes);
            let _ = hk_fw::firmware::parse_pptt(&bytes);
            let _ = hk_fw::firmware::parse_dmar(&bytes);
            let _ = hk_fw::firmware::parse_ivrs(&bytes);
            let _ = hk_fw::firmware::parse_hpet(&bytes);
            let _ = hk_fw::firmware::parse_facs(&bytes);
            let _ = hk_fw::firmware::parse_s5(&bytes);
            let _ = hk_fw::dtb::parse(&bytes);
        })?;

        state = next(state);
        let disk_bytes = (state as usize % (128 * 1024 / SECTOR_SIZE)) * SECTOR_SIZE;
        let image = random_bytes(state, disk_bytes);
        run_case(iteration, arguments.seed, || {
            let Some(disk) = MemoryDisk::from_data(image.clone(), SECTOR_SIZE) else {
                return;
            };
            if let Ok(fs) = HolyFs::mount(disk) {
                let _ = fs.check();
                let _ = fs.read_document("/");
            }
        })?;
    }
    println!(
        "fuzz：{} 次迭代通过（seed {:#x}）",
        arguments.iterations, arguments.seed
    );
    Ok(())
}

fn run_case(iteration: u64, seed: u64, case: impl FnOnce() + std::panic::UnwindSafe) -> Result<()> {
    if catch_unwind(AssertUnwindSafe(case)).is_err() {
        return Err(Failure::new(format!(
            "fuzz 失败：seed={seed:#x} iteration={iteration} 发生 panic"
        )));
    }
    Ok(())
}

fn random_bytes(seed: u64, length: usize) -> Vec<u8> {
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
