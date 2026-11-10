//! x86_64 的 HAL `Platform` 适配。
//!
//! 把既有的具体模块（串口、TSC/LAPIC 定时器、legacy PCI、DMA 分配器、direct map、
//! IOAPIC 与页表）包装成 [`hk_hal::Platform`] 的 trait 对象，作为多架构 HAL
//! 冻结的 x86_64 实现。启动路径仍复用现有模块，平台只提供统一入口。

use hk_base::Moment;
use hk_hal::{
    Arch, BootInfo, Console, CpuId, DmaOps, Entropy, InterruptController, MAX_CPUS, PhysAddr,
    PhysicalAccess, PhysicalError, Platform, PlatformError, Timer,
};
use hk_test::{ExitCode, exit};

use crate::port::{inb, outb};
use crate::serial::{COM1, Serial};
use crate::{cpu, paging};

/// x86_64 平台。
#[derive(Clone, Copy, Debug, Default)]
pub struct X86Platform;

/// 全局平台实例。
pub static PLATFORM: X86Platform = X86Platform;

impl Platform for X86Platform {
    fn name(&self) -> &'static str {
        "x86_64-qemu"
    }

    fn arch(&self) -> Arch {
        Arch::X86_64
    }

    fn cpu_count(&self) -> usize {
        (1 + hk_smp::AP_POOL.online()).min(MAX_CPUS)
    }

    fn init(&self, boot: &BootInfo) -> Result<(), PlatformError> {
        boot.validate().map_err(PlatformError::BootInfo)
    }

    fn init_cpu(&self, cpu: CpuId) -> Result<(), PlatformError> {
        if cpu.get() as usize >= MAX_CPUS {
            return Err(PlatformError::CpuOutOfRange { cpu });
        }
        Ok(())
    }

    fn shutdown(&self) -> ! {
        if crate::bootstrap::power_off() {
            loop {
                x86_64::instructions::hlt();
            }
        }
        exit(ExitCode::Success)
    }

    fn reboot(&self) -> ! {
        if crate::bootstrap::reset() {
            loop {
                x86_64::instructions::hlt();
            }
        }
        // 8042 复位脉冲：等待输入缓冲为空后写入 reset 命令。
        // SAFETY: 0x64/0x60 是标准键盘控制器端口；复位成功后不会返回。
        unsafe {
            for _ in 0..100_000 {
                if inb(0x64) & 0x02 == 0 {
                    break;
                }
            }
            outb(0x64, 0xFE);
        }
        loop {
            x86_64::instructions::hlt();
        }
    }

    fn irq(&self) -> &dyn InterruptController {
        &crate::bootstrap::CONTROLLER
    }

    fn timer(&self) -> &dyn Timer {
        &TIMER
    }

    fn console(&self) -> &dyn Console {
        &CONSOLE
    }

    fn pci(&self) -> Option<&dyn hk_device::PciConfig> {
        Some(&crate::pci::PCI)
    }

    fn dma(&self) -> &dyn DmaOps {
        &crate::dma::DMA
    }

    fn physical(&self) -> &dyn PhysicalAccess {
        &PHYSICAL
    }

    fn entropy(&self) -> &dyn Entropy {
        &crate::entropy::ENTROPY
    }

    fn iommu(&self) -> Option<&dyn hk_hal::Iommu> {
        Some(&crate::iommu::IOMMU)
    }

    fn cache(&self) -> Option<&dyn hk_hal::CacheOps> {
        Some(&crate::cache::CACHE)
    }

    fn temperature_celsius(&self) -> Option<u8> {
        // CPUID.06H:EAX[0] 表示数字温度传感器可用。
        if core::arch::x86_64::__cpuid(6).eax & 1 == 0 {
            return None;
        }
        const IA32_THERM_STATUS: u32 = 0x19C;
        const IA32_TEMPERATURE_TARGET: u32 = 0x1A2;
        // SAFETY: CPUID 已确认 DTS；读取当前 CPU 的热状态 MSR。
        let (target, status) = unsafe {
            (
                x86_64::registers::model_specific::Msr::new(IA32_TEMPERATURE_TARGET).read(),
                x86_64::registers::model_specific::Msr::new(IA32_THERM_STATUS).read(),
            )
        };
        let tj_max = ((target >> 16) & 0xFF) as u8;
        let reading = ((status >> 16) & 0x7F) as u8;
        Some(tj_max.saturating_sub(reading))
    }

    fn cpu_topology(&self, cpu: CpuId) -> Option<hk_hal::CpuTopology> {
        crate::bootstrap::cpu_topology(cpu.get() as usize)
    }
}

/// 串口控制台。
pub struct X86Console;

static CONSOLE: X86Console = X86Console;

impl Console for X86Console {
    fn write_str(&self, text: &str) {
        Serial::new(COM1).write_str(text);
    }
}

/// TSC 单调时钟与 LAPIC 一次性定时器。
pub struct X86Timer;

static TIMER: X86Timer = X86Timer;

impl Timer for X86Timer {
    fn frequency(&self) -> u64 {
        let tsc = crate::timing::tsc_frequency();
        if tsc != 0 {
            tsc
        } else {
            crate::hpet::frequency()
        }
    }

    fn now(&self) -> Moment {
        if crate::timing::tsc_frequency() != 0 {
            Moment::from_ticks(crate::timing::rdtsc())
        } else {
            Moment::from_ticks(crate::hpet::now())
        }
    }

    fn set_oneshot(&self, deadline: Moment) {
        if crate::timing::tsc_frequency() == 0 {
            return;
        }
        let frequency = self.frequency();
        if frequency == 0 {
            return;
        }
        let now = crate::timing::rdtsc();
        let delta = deadline.ticks().saturating_sub(now);
        let micros = (u128::from(delta) * 1_000_000 / u128::from(frequency)) as u64;
        cpu::arm_oneshot(micros);
    }

    fn cancel(&self) {
        cpu::disarm_timer();
    }
}

/// direct map 物理内存访问。
pub struct X86Physical;

static PHYSICAL: X86Physical = X86Physical;

impl PhysicalAccess for X86Physical {
    fn read(&self, address: PhysAddr, buffer: &mut [u8]) -> Result<(), PhysicalError> {
        let base = paging::direct_map_base();
        if base == 0 {
            return Err(PhysicalError::OutOfRange);
        }
        let source = (base + address.get()) as *const u8;
        // SAFETY: 调用方保证地址落在 direct map 覆盖的物理内存内。
        unsafe { core::ptr::copy_nonoverlapping(source, buffer.as_mut_ptr(), buffer.len()) };
        Ok(())
    }

    fn write(&self, address: PhysAddr, bytes: &[u8]) -> Result<(), PhysicalError> {
        let base = paging::direct_map_base();
        if base == 0 {
            return Err(PhysicalError::OutOfRange);
        }
        let target = (base + address.get()) as *mut u8;
        // SAFETY: 调用方保证地址落在 direct map 覆盖的物理内存内。
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len()) };
        Ok(())
    }

    fn copy(&self, from: PhysAddr, to: PhysAddr, len: usize) -> Result<(), PhysicalError> {
        let base = paging::direct_map_base();
        if base == 0 {
            return Err(PhysicalError::OutOfRange);
        }
        let source = (base + from.get()) as *const u8;
        let target = (base + to.get()) as *mut u8;
        // SAFETY: 调用方保证两端都落在 direct map 覆盖的物理内存内且不重叠。
        unsafe { core::ptr::copy_nonoverlapping(source, target, len) };
        Ok(())
    }
}
