//! Hosted 平台实现。

use hk_base::{CivilClock, CivilTime};
use hk_device::PciConfig;
use hk_hal::{
    Arch, BootInfo, Console, CpuId, DmaOps, Entropy, InterruptController, Mmu, PhysAddr, PhysRange,
    PhysicalAccess, Platform, PlatformError, Timer,
};

use crate::boot::{HOSTED_RAM_BASE, HOSTED_RAM_SIZE, boot_info};
use crate::config::HostedConfig;
use crate::console::HostedConsole;
use crate::dma::HostedDma;
use crate::entropy::HostedEntropy;
use crate::irq::HostedInterruptController;
use crate::mmu::HostedMmu;
use crate::physical::HostedMemory;
use crate::timer::HostedTimer;

/// Hosted 平台。
///
/// 平台把虚拟 CPU、虚拟时钟、虚拟地址空间与模拟设备打包在一起，供微内核与
/// 用户态服务器在普通进程中运行。
#[derive(Debug)]
pub struct HostedPlatform {
    config: HostedConfig,
    boot: BootInfo,
    console: HostedConsole,
    timer: HostedTimer,
    irq: HostedInterruptController,
    mmu: HostedMmu,
    memory: HostedMemory,
    dma: HostedDma,
    entropy: HostedEntropy,
    civil: CivilClock,
}

/// Hosted 模拟民用时间的起点。
const HOSTED_CIVIL_ANCHOR: CivilTime = CivilTime {
    year: 2000,
    month: 1,
    day: 1,
    hour: 0,
    minute: 0,
    second: 0,
    nanosecond: 0,
};

impl HostedPlatform {
    /// 按配置启动 Hosted 平台。
    ///
    /// # Panics
    /// `config.cpu_count` 为 0 时 panic：CPU 数量是平台的硬性约束。
    pub fn boot(config: HostedConfig) -> Self {
        assert!(config.cpu_count > 0, "Hosted 平台至少需要一个逻辑 CPU");
        let timer = HostedTimer::new(config.deterministic);
        let civil = CivilClock::new(HOSTED_CIVIL_ANCHOR, timer.now().ticks(), timer.frequency());
        Self {
            config,
            boot: boot_info(),
            console: HostedConsole::new(),
            timer,
            irq: HostedInterruptController::new(),
            mmu: HostedMmu::new(),
            memory: HostedMemory::new(PhysRange::new(
                PhysAddr::new(HOSTED_RAM_BASE),
                HOSTED_RAM_SIZE,
            )),
            dma: HostedDma::new(),
            entropy: HostedEntropy::new(config.seed),
            civil,
        }
    }

    /// 返回当前模拟民用时间。
    pub fn civil_time(&self) -> Option<CivilTime> {
        Some(self.civil.now(self.timer.now().ticks()))
    }

    /// 返回启动信息。
    pub fn boot_info(&self) -> BootInfo {
        self.boot
    }

    /// 返回平台配置。
    pub fn config(&self) -> HostedConfig {
        self.config
    }

    /// 返回虚拟时钟，用于确定性时间推进。
    pub fn virtual_timer(&self) -> &HostedTimer {
        &self.timer
    }

    /// 返回虚拟地址空间表。
    pub fn virtual_mmu(&self) -> &HostedMmu {
        &self.mmu
    }

    /// 返回虚拟中断控制器。
    pub fn virtual_irq(&self) -> &HostedInterruptController {
        &self.irq
    }

    /// 返回模拟物理内存。
    pub fn memory(&self) -> &HostedMemory {
        &self.memory
    }
}

impl Platform for HostedPlatform {
    fn name(&self) -> &'static str {
        "hosted"
    }

    fn arch(&self) -> Arch {
        Arch::Hosted
    }

    fn cpu_count(&self) -> usize {
        self.config.cpu_count
    }

    fn init(&self, boot: &BootInfo) -> Result<(), PlatformError> {
        boot.validate().map_err(PlatformError::BootInfo)
    }

    fn init_cpu(&self, cpu: CpuId) -> Result<(), PlatformError> {
        if cpu.get() as usize >= self.config.cpu_count {
            return Err(PlatformError::CpuOutOfRange { cpu });
        }
        Ok(())
    }

    fn shutdown(&self) -> ! {
        self.console.write_str("hosted: 关闭系统\n");
        std::process::exit(0);
    }

    fn reboot(&self) -> ! {
        self.console.write_str("hosted: 重启系统\n");
        std::process::exit(0);
    }

    fn mmu(&self) -> &dyn Mmu {
        &self.mmu
    }

    fn irq(&self) -> &dyn InterruptController {
        &self.irq
    }

    fn timer(&self) -> &dyn Timer {
        &self.timer
    }

    fn console(&self) -> &dyn Console {
        &self.console
    }

    fn pci(&self) -> Option<&dyn PciConfig> {
        None
    }

    fn dma(&self) -> &dyn DmaOps {
        &self.dma
    }

    fn physical(&self) -> &dyn PhysicalAccess {
        &self.memory
    }

    fn entropy(&self) -> &dyn Entropy {
        &self.entropy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn platform() -> HostedPlatform {
        HostedPlatform::boot(HostedConfig::new(2, 1, true))
    }

    #[test]
    fn civil_time_advances_with_virtual_clock() {
        let platform = platform();
        let start = platform.civil_time().expect("民用时间");
        platform
            .timer
            .advance(hk_base::Duration::from_ticks(1_000_000_000));
        let next = platform.civil_time().expect("民用时间");
        assert_eq!(next.second, start.second + 1);
    }

    #[test]
    fn boots_with_valid_boot_info() {
        let platform = platform();

        assert_eq!(platform.name(), "hosted");
        assert_eq!(platform.arch(), Arch::Hosted);
        assert_eq!(platform.cpu_count(), 2);
        assert_eq!(platform.init(&platform.boot_info()), Ok(()));
    }

    #[test]
    fn rejects_cpu_out_of_range() {
        let platform = platform();

        assert_eq!(platform.init_cpu(CpuId::new(1)), Ok(()));
        assert_eq!(
            platform.init_cpu(CpuId::new(2)),
            Err(PlatformError::CpuOutOfRange { cpu: CpuId::new(2) })
        );
    }

    #[test]
    fn exposes_no_pci_config() {
        let platform = platform();
        assert!(platform.pci().is_none());
    }

    #[test]
    fn rejects_corrupted_boot_info() {
        let platform = platform();
        let mut boot = platform.boot_info();
        boot.magic = 0;

        assert_eq!(
            platform.init(&boot),
            Err(PlatformError::BootInfo(hk_hal::BootInfoError::BadMagic {
                found: 0
            }))
        );
    }
}
