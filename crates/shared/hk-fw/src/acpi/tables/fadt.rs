use super::{SIGNATURE_FADT, table_length, u8_at, u16_at, u32_at, u64_at};

/// FADT 电源管理与通用地址。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GenericAddress {
    /// 地址空间编号（0 = 系统内存，1 = 系统 I/O）。
    pub space: u8,
    /// 位宽。
    pub bit_width: u8,
    /// 位偏移。
    pub bit_offset: u8,
    /// 访问宽度。
    pub access_width: u8,
    /// 地址。
    pub address: u64,
}

impl GenericAddress {
    pub(super) fn parse(bytes: &[u8], offset: usize) -> Option<Self> {
        Some(Self {
            space: u8_at(bytes, offset)?,
            bit_width: u8_at(bytes, offset + 1)?,
            bit_offset: u8_at(bytes, offset + 2)?,
            access_width: u8_at(bytes, offset + 3)?,
            address: u64_at(bytes, offset + 4)?,
        })
    }
}

/// 固定 ACPI 描述表（FADT）的电源相关视图。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Fadt {
    /// 固件控制结构（FACS）物理地址。
    pub firmware_ctrl: u32,
    /// 扩展固件控制结构地址（FADT rev 6+）。
    pub x_firmware_ctrl: u64,
    /// DSDT 物理地址（32 位字段；0 表示使用扩展字段）。
    pub dsdt: u64,
    /// SCI 中断号。
    pub sci_interrupt: u16,
    /// PM1a 事件块 I/O 端口。
    pub pm1a_event: u32,
    /// PM1b 事件块 I/O 端口。
    pub pm1b_event: u32,
    /// PM1 事件寄存器长度（字节）。
    pub pm1_event_length: u8,
    /// GPE0 块 I/O 端口。
    pub gpe0_block: u32,
    /// GPE0 块长度。
    pub gpe0_length: u8,
    /// GPE1 块 I/O 端口。
    pub gpe1_block: u32,
    /// GPE1 块长度。
    pub gpe1_length: u8,
    /// GPE1 基址。
    pub gpe1_base: u8,
    /// PM1a 控制块 I/O 端口。
    pub pm1a_control: u32,
    /// PM1b 控制块 I/O 端口；0 表示不存在。
    pub pm1b_control: u32,
    /// PM1 控制寄存器长度（字节）。
    pub pm1_control_length: u8,
    /// 复位寄存器。
    pub reset_register: GenericAddress,
    /// 复位值。
    pub reset_value: u8,
    /// ACPI 主版本。
    pub major: u8,
    /// ACPI 次版本。
    pub minor: u8,
    /// S5 休眠控制寄存器（FADT rev 6+）。
    pub sleep_control: u32,
    /// S5 休眠状态寄存器（FADT rev 6+）。
    pub sleep_status: u32,
}

/// 从 FADT 字节解析电源字段。
pub fn parse_fadt(bytes: &[u8]) -> Option<Fadt> {
    let length = table_length(bytes, SIGNATURE_FADT)?;
    let bytes = &bytes[..length];
    if bytes.len() < 116 {
        return None;
    }
    Some(Fadt {
        firmware_ctrl: u32_at(bytes, 36)?,
        x_firmware_ctrl: if bytes.len() >= 140 {
            u64_at(bytes, 132).unwrap_or(0)
        } else {
            0
        },
        dsdt: if bytes.len() >= 148 {
            let extended = u64_at(bytes, 140).unwrap_or(0);
            if extended != 0 {
                extended
            } else {
                u64::from(u32_at(bytes, 40)?)
            }
        } else {
            u64::from(u32_at(bytes, 40)?)
        },
        sci_interrupt: u16_at(bytes, 49)?,
        pm1a_event: u32_at(bytes, 56)?,
        pm1b_event: u32_at(bytes, 60)?,
        pm1_event_length: u8_at(bytes, 88)?,
        gpe0_block: u32_at(bytes, 80)?,
        gpe0_length: u8_at(bytes, 92)?,
        gpe1_block: u32_at(bytes, 84)?,
        gpe1_length: u8_at(bytes, 93)?,
        gpe1_base: u8_at(bytes, 94)?,
        pm1a_control: u32_at(bytes, 64)?,
        pm1b_control: u32_at(bytes, 68)?,
        pm1_control_length: u8_at(bytes, 89)?,
        reset_register: GenericAddress::parse(bytes, 116).unwrap_or_default(),
        reset_value: u8_at(bytes, 128)?,
        major: u8_at(bytes, 8)?,
        minor: u8_at(bytes, 131).unwrap_or(0),
        sleep_control: if bytes.len() >= 248 {
            u32_at(bytes, 244).unwrap_or(0)
        } else {
            0
        },
        sleep_status: if bytes.len() >= 252 {
            u32_at(bytes, 248).unwrap_or(0)
        } else {
            0
        },
    })
}
