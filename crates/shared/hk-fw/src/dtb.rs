//! 设备树（DTB）解析。
//!
//! 使用外部 `hermit-dtb` 遍历节点，产出内存、CPU 与通用设备描述。节点路径与
//! 兼容字符串保持设备树原样，具体设备语义由平台的 `Platform`/驱动层决定。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use hermit_dtb::Dtb;

use crate::{Cpu, Device, MemoryKind, MemoryRegion};

/// DTB 头部长度。
const HEADER_LEN: usize = 40;
/// DTB 魔数。
const DTB_MAGIC: u32 = 0xD00D_FEED;

/// 设备树解析结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DtbInfo {
    /// 可分配与保留内存区域。
    pub memory: Vec<MemoryRegion>,
    /// CPU 列表。
    pub cpus: Vec<Cpu>,
    /// 带 `compatible` 的设备节点。
    pub devices: Vec<Device>,
    /// 固件指定的启动 CPU 硬件标识。
    pub boot_cpu: u64,
}

impl DtbInfo {
    /// 返回声明了给定兼容字符串的设备。
    pub fn find_compatible<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Device> {
        self.devices
            .iter()
            .filter(move |device| device.is_compatible(name))
    }

    /// 返回设备树声明的平台中性 IOMMU 单元。
    pub fn iommu_info(&self) -> crate::IommuInfo {
        crate::IommuInfo::from_dtb(self)
    }

    /// 返回 ACPI/DTB 共享的规范化固件描述。
    pub fn firmware_info(&self) -> crate::FirmwareInfo {
        crate::FirmwareInfo::from_dtb(self)
    }
}

/// 解析设备树字节。
///
/// `bytes` 必须覆盖完整的 DTB（`totalsize` 字节）。损坏的头部返回 `None`；
/// 头部合法但节点结构异常时 `hermit-dtb` 可能 panic，启动期固件输入可接受。
pub fn parse(bytes: &[u8]) -> Option<DtbInfo> {
    if bytes.len() < HEADER_LEN {
        return None;
    }
    if be32(bytes, 0)? != DTB_MAGIC {
        return None;
    }
    let total = be32(bytes, 4)? as usize;
    if total < HEADER_LEN || total > bytes.len() {
        return None;
    }
    let struct_offset = be32(bytes, 8)? as usize;
    let struct_size = be32(bytes, 36)? as usize;
    let strings_offset = be32(bytes, 12)? as usize;
    let strings_size = be32(bytes, 32)? as usize;
    if struct_offset.checked_add(struct_size)? > total
        || strings_offset.checked_add(strings_size)? > total
    {
        return None;
    }
    let boot_cpu = u64::from(be32(bytes, 28)?);

    // SAFETY: 头部已校验，`bytes` 覆盖 `totalsize`；`hermit-dtb` 只按头部偏移
    // 读取 struct/strings 两个区间，均已确认落在 `bytes` 内。
    let dtb = unsafe { Dtb::from_raw(bytes.as_ptr()) }?;

    let address_cells = property_u32(&dtb, "/", "#address-cells").unwrap_or(2);
    let size_cells = property_u32(&dtb, "/", "#size-cells").unwrap_or(1);

    let mut info = DtbInfo {
        boot_cpu,
        ..DtbInfo::default()
    };

    for name in dtb.enum_subnodes("/") {
        if name == "memory" || name.starts_with("memory@") {
            let path = format!("/{name}");
            if let Some(reg) = dtb.get_property(&path, "reg") {
                for (base, len) in cells(reg, address_cells, size_cells) {
                    info.memory.push(MemoryRegion {
                        base,
                        len,
                        kind: MemoryKind::Usable,
                    });
                }
            }
        } else if name == "reserved-memory" {
            parse_reserved(&dtb, address_cells, size_cells, &mut info.memory);
        }
    }

    parse_cpus(&dtb, &mut info.cpus);
    collect_devices(&dtb, "/", 2, address_cells, size_cells, &mut info.devices);

    Some(info)
}

fn parse_reserved(
    dtb: &Dtb<'_>,
    address_cells: u32,
    size_cells: u32,
    memory: &mut Vec<MemoryRegion>,
) {
    for name in dtb.enum_subnodes("/reserved-memory") {
        if name.is_empty() {
            continue;
        }
        let path = format!("/reserved-memory/{name}");
        if let Some(reg) = dtb.get_property(&path, "reg") {
            for (base, len) in cells(reg, address_cells, size_cells) {
                memory.push(MemoryRegion {
                    base,
                    len,
                    kind: MemoryKind::Reserved,
                });
            }
        }
    }
}

fn parse_cpus(dtb: &Dtb<'_>, cpus: &mut Vec<Cpu>) {
    let address_cells = property_u32(dtb, "/cpus", "#address-cells").unwrap_or(2);
    for name in dtb.enum_subnodes("/cpus") {
        if name.is_empty() || name.starts_with("cpu-map") {
            continue;
        }
        let path = format!("/cpus/{name}");
        let hardware_id = dtb
            .get_property(&path, "reg")
            .and_then(|reg| integer(reg, address_cells))
            .unwrap_or(0);
        let enabled = match dtb.get_property(&path, "status") {
            Some(status) => !cstr(status).starts_with("disabled"),
            None => true,
        };
        let id = cpus.len() as u64;
        cpus.push(Cpu {
            id,
            hardware_id,
            enabled,
        });
    }
}

fn collect_devices(
    dtb: &Dtb<'_>,
    parent: &str,
    depth: u32,
    address_cells: u32,
    size_cells: u32,
    devices: &mut Vec<Device>,
) {
    for name in dtb.enum_subnodes(parent) {
        if name.is_empty() {
            continue;
        }
        let path = if parent == "/" {
            format!("/{name}")
        } else {
            format!("{parent}/{name}")
        };
        if let Some(compatible) = dtb.get_property(&path, "compatible") {
            let compatible = compatible_list(compatible);
            if !compatible.is_empty() {
                let regions = dtb
                    .get_property(&path, "reg")
                    .map(|reg| cells(reg, address_cells, size_cells))
                    .unwrap_or_default();
                let irqs = dtb
                    .get_property(&path, "interrupts")
                    .map(|data| {
                        data.as_chunks::<4>()
                            .0
                            .iter()
                            .filter_map(|chunk| be32(chunk, 0))
                            .collect()
                    })
                    .unwrap_or_default();
                devices.push(Device {
                    path: path.clone(),
                    compatible,
                    regions,
                    irqs,
                });
            }
        }
        if depth > 1 && !skip_subtree(name) {
            collect_devices(dtb, &path, depth - 1, address_cells, size_cells, devices);
        }
    }
}

fn skip_subtree(name: &str) -> bool {
    matches!(
        name,
        "cpus" | "chosen" | "aliases" | "memory" | "reserved-memory"
    ) || name.starts_with("memory@")
}

fn property_u32(dtb: &Dtb<'_>, path: &str, name: &str) -> Option<u32> {
    let bytes = dtb.get_property(path, name)?;
    be32(bytes, 0)
}

/// 按大端读取 `(地址, 长度)` 单元对。
fn cells(data: &[u8], address_cells: u32, size_cells: u32) -> Vec<(u64, u64)> {
    let stride = (address_cells + size_cells) as usize * 4;
    if stride == 0 {
        return Vec::new();
    }
    let mut result = Vec::new();
    for chunk in data.chunks_exact(stride) {
        let address_bytes = &chunk[..address_cells as usize * 4];
        let size_bytes = &chunk[address_cells as usize * 4..];
        let Some(address) = integer(address_bytes, address_cells) else {
            continue;
        };
        let Some(size) = integer(size_bytes, size_cells) else {
            continue;
        };
        result.push((address, size));
    }
    result
}

/// 把最多 8 字节的大端单元数组读成整数；单元数为 0 时返回 0。
fn integer(data: &[u8], cell_count: u32) -> Option<u64> {
    let bytes = cell_count as usize * 4;
    if bytes == 0 {
        return Some(0);
    }
    if data.len() < bytes || bytes > 8 {
        return None;
    }
    let mut value = 0_u64;
    for chunk in data[..bytes].as_chunks::<4>().0 {
        value = (value << 32) | u64::from(u32::from_be_bytes(*chunk));
    }
    Some(value)
}

fn be32(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        data.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn cstr(data: &[u8]) -> &str {
    let end = data
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(data.len());
    core::str::from_utf8(&data[..end]).unwrap_or("")
}

fn compatible_list(data: &[u8]) -> Vec<String> {
    data.split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .filter_map(|entry| core::str::from_utf8(entry).ok())
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;

    struct Builder {
        strings: Vec<u8>,
        offsets: BTreeMap<String, u32>,
        structure: Vec<u8>,
    }

    impl Builder {
        fn new() -> Self {
            Self {
                strings: Vec::new(),
                offsets: BTreeMap::new(),
                structure: Vec::new(),
            }
        }

        fn string(&mut self, name: &str) -> u32 {
            if let Some(offset) = self.offsets.get(name) {
                return *offset;
            }
            let offset = self.strings.len() as u32;
            self.strings.extend_from_slice(name.as_bytes());
            self.strings.push(0);
            self.offsets.insert(String::from(name), offset);
            offset
        }

        fn token(&mut self, token: u32) {
            self.structure.extend_from_slice(&token.to_be_bytes());
        }

        fn begin(&mut self, name: &str) {
            self.token(1);
            self.structure.extend_from_slice(name.as_bytes());
            self.structure.push(0);
            while !self.structure.len().is_multiple_of(4) {
                self.structure.push(0);
            }
        }

        fn end(&mut self) {
            self.token(2);
        }

        fn prop(&mut self, name: &str, data: &[u8]) {
            self.token(3);
            self.structure
                .extend_from_slice(&(data.len() as u32).to_be_bytes());
            let offset = self.string(name);
            self.structure.extend_from_slice(&offset.to_be_bytes());
            self.structure.extend_from_slice(data);
            while !self.structure.len().is_multiple_of(4) {
                self.structure.push(0);
            }
        }

        fn u32_prop(&mut self, name: &str, value: u32) {
            self.prop(name, &value.to_be_bytes());
        }

        fn build(mut self) -> Vec<u8> {
            self.token(9);
            let struct_size = self.structure.len() as u32;
            let strings_offset = HEADER_LEN as u32 + struct_size;
            let total = strings_offset + self.strings.len() as u32;
            let mut out = Vec::with_capacity(total as usize);
            out.extend_from_slice(&DTB_MAGIC.to_be_bytes());
            out.extend_from_slice(&total.to_be_bytes());
            out.extend_from_slice(&(HEADER_LEN as u32).to_be_bytes());
            out.extend_from_slice(&strings_offset.to_be_bytes());
            out.extend_from_slice(&0_u32.to_be_bytes());
            out.extend_from_slice(&17_u32.to_be_bytes());
            out.extend_from_slice(&16_u32.to_be_bytes());
            out.extend_from_slice(&0_u32.to_be_bytes());
            out.extend_from_slice(&(self.strings.len() as u32).to_be_bytes());
            out.extend_from_slice(&struct_size.to_be_bytes());
            out.extend_from_slice(&self.structure);
            out.extend_from_slice(&self.strings);
            out
        }
    }

    fn sample() -> Vec<u8> {
        let mut builder = Builder::new();
        builder.begin("");
        builder.u32_prop("#address-cells", 2);
        builder.u32_prop("#size-cells", 2);

        builder.begin("memory@40000000");
        builder.prop("device_type", b"memory\0");
        builder.prop(
            "reg",
            &[0, 0, 0, 0, 0x40, 0, 0, 0, 0, 0, 0, 0, 0x08, 0, 0, 0],
        );
        builder.end();

        builder.begin("cpus");
        builder.u32_prop("#address-cells", 1);
        builder.u32_prop("#size-cells", 0);
        builder.begin("cpu@0");
        builder.prop("reg", &0_u32.to_be_bytes());
        builder.prop("status", b"okay\0");
        builder.end();
        builder.end();

        builder.begin("intc");
        builder.prop("compatible", b"arm,gic-400\0");
        builder.prop(
            "reg",
            &[
                0, 0, 0, 0, 0x08, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10, 0, 0, 0, 0, 0, 0x08, 0x01, 0, 0,
                0, 0, 0, 0, 0, 0, 0x20, 0,
            ],
        );
        builder.end();

        builder.end();
        builder.build()
    }

    #[test]
    fn parses_memory_cpus_and_devices() {
        let bytes = sample();
        let info = parse(&bytes).expect("解析成功");
        assert_eq!(
            info.memory,
            [MemoryRegion {
                base: 0x4000_0000,
                len: 0x0800_0000,
                kind: MemoryKind::Usable,
            }]
        );
        assert_eq!(info.cpus.len(), 1);
        assert_eq!(info.cpus[0].hardware_id, 0);
        assert!(info.cpus[0].enabled);
        let gic = info
            .find_compatible("arm,gic-400")
            .next()
            .expect("找到 GIC");
        assert_eq!(gic.regions[0], (0x0800_0000, 0x1000));
        assert_eq!(gic.regions[1], (0x0801_0000, 0x2000));
    }

    #[test]
    fn rejects_bad_magic() {
        assert!(parse(&[0_u8; HEADER_LEN]).is_none());
    }
}
