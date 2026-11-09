//! AHCI HBA 寄存器访问与端口枚举。

/// HBA 全局寄存器偏移。
const HBA_CAP: u64 = 0x00;
const HBA_GHC: u64 = 0x04;
const HBA_PI: u64 = 0x0C;
const PORT_BASE: u64 = 0x100;
const PORT_STRIDE: u64 = 0x80;

/// 端口寄存器偏移。
const PX_CLB: u64 = 0x00;
const PX_CLBU: u64 = 0x04;
const PX_FB: u64 = 0x08;
const PX_FBU: u64 = 0x0C;
const PX_IS: u64 = 0x10;
const PX_IE: u64 = 0x14;
const PX_CMD: u64 = 0x18;
const PX_TFD: u64 = 0x20;
const PX_SIG: u64 = 0x24;
const PX_SSTS: u64 = 0x28;
const PX_SCTL: u64 = 0x2C;
const PX_SERR: u64 = 0x30;
const PX_CI: u64 = 0x38;

const CMD_ST: u32 = 1 << 0;
const CMD_FRE: u32 = 1 << 4;
const GHC_AE: u32 = 1 << 31;
const GHC_HR: u32 = 1 << 0;
const GHC_IE: u32 = 1 << 1;
const TFD_ERR: u32 = 1 << 0;
const TFD_BSY: u32 = 1 << 7;
const TFD_DRQ: u32 = 1 << 3;

const SATA_SIG_ATA: u32 = 0x0000_0101;

/// HBA 寄存器页。
pub struct Hba {
    base: u64,
}

impl Hba {
    /// 以 DeviceGrant 映射出的虚拟基址构造。
    pub const fn new(base: u64) -> Self {
        Self { base }
    }

    fn read32(&self, offset: u64) -> u32 {
        // SAFETY: 基址来自 DeviceGrant 的 AHCI BAR 映射。
        unsafe { core::ptr::read_volatile((self.base + offset) as *const u32) }
    }

    fn write32(&self, offset: u64, value: u32) {
        // SAFETY: 同上；只写授权设备页。
        unsafe { core::ptr::write_volatile((self.base + offset) as *mut u32, value) }
    }

    fn port_offset(index: u16, field: u64) -> u64 {
        PORT_BASE + u64::from(index) * PORT_STRIDE + field
    }

    /// 复位 HBA。
    pub fn reset(&self) {
        self.write32(HBA_GHC, self.read32(HBA_GHC) | GHC_AE);
        self.write32(HBA_GHC, self.read32(HBA_GHC) | GHC_HR);
        for _ in 0..1_000_000 {
            if self.read32(HBA_GHC) & GHC_HR == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        self.write32(HBA_GHC, self.read32(HBA_GHC) | GHC_AE);
    }

    /// 返回已实现的端口位图。
    pub fn implemented_ports(&self) -> u32 {
        self.read32(HBA_PI)
    }

    /// 返回 HBA 报告的最大端口号（含）。
    pub fn max_port(&self) -> u16 {
        ((self.read32(HBA_CAP) & 0x1F) as u16).min(31)
    }

    /// 判断端口是否连接了 SATA 磁盘。
    pub fn is_disk(&self, index: u16) -> bool {
        let base = Self::port_offset(index, 0);
        self.read32(base + PX_SSTS) & 0x0F == 3 && self.read32(base + PX_SIG) == SATA_SIG_ATA
    }

    /// 停止并重新启动端口，安装命令列表与 FIS 地址。
    pub fn start_port(&self, index: u16, command_list_phys: u64, fis_phys: u64) -> Option<()> {
        let command = Self::port_offset(index, PX_CMD);
        self.write32(command, self.read32(command) & !(CMD_ST | CMD_FRE));
        for _ in 0..1_000_000 {
            if self.read32(command) & (CMD_ST | CMD_FRE) == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        let control = Self::port_offset(index, PX_SCTL);
        self.write32(control, 1);
        for _ in 0..1_000_000 {
            core::hint::spin_loop();
        }
        self.write32(control, 0);
        for _ in 0..1_000_000 {
            core::hint::spin_loop();
        }
        self.write32(Self::port_offset(index, PX_SERR), u32::MAX);
        self.write32(Self::port_offset(index, PX_CLB), command_list_phys as u32);
        self.write32(
            Self::port_offset(index, PX_CLBU),
            (command_list_phys >> 32) as u32,
        );
        self.write32(Self::port_offset(index, PX_FB), fis_phys as u32);
        self.write32(Self::port_offset(index, PX_FBU), (fis_phys >> 32) as u32);
        self.write32(Self::port_offset(index, PX_CMD), CMD_FRE | CMD_ST);
        Some(())
    }

    /// 启用端口中断与 HBA 全局中断。
    pub fn enable_interrupts(&self, index: u16) {
        self.write32(Self::port_offset(index, PX_IE), 0x7FFF_FFFF);
        self.write32(HBA_GHC, self.read32(HBA_GHC) | GHC_IE);
    }

    /// 读取并清除端口中断状态。
    pub fn acknowledge(&self, index: u16) -> u32 {
        let offset = Self::port_offset(index, PX_IS);
        let status = self.read32(offset);
        self.write32(offset, status);
        status
    }

    /// 提交命令槽。
    pub fn issue(&self, index: u16, slot: usize) {
        self.write32(Self::port_offset(index, PX_CI), 1 << slot);
    }

    /// 返回端口仍在执行的命令位图。
    pub fn commands_issued(&self, index: u16) -> u32 {
        self.read32(Self::port_offset(index, PX_CI))
    }

    /// 判断端口当前是否处于错误状态。
    pub fn port_error(&self, index: u16) -> bool {
        let tfd = self.read32(Self::port_offset(index, PX_TFD));
        tfd & (TFD_ERR | TFD_BSY | TFD_DRQ) != 0
    }
}
