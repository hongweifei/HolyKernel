//! 设备授权：用户态驱动可获得的 MMIO、I/O 与 IRQ 资源描述。

use crate::pci::PciAddress;
use hk_base::{PhysAddr, PhysRange};

/// MMIO 物理范围。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MmioRange {
    /// 物理基址。
    pub base: u64,
    /// 长度。
    pub len: u64,
}

/// 端口 I/O 范围。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoRange {
    /// 端口基址。
    pub base: u16,
    /// 长度。
    pub len: u16,
}

/// 设备授权描述。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceGrant {
    /// 配置空间地址。
    pub address: PciAddress,
    /// 厂商编号。
    pub vendor: u16,
    /// 设备编号。
    pub device: u16,
    /// MMIO 范围。
    pub mmio: [Option<MmioRange>; 6],
    /// 端口 I/O 范围。
    pub io: [Option<IoRange>; 6],
    /// 授权给驱动的连续 DMA 窗口。
    pub dma: Option<PhysRange>,
    /// 中断线。
    pub irq: Option<u8>,
    /// capability ID。
    pub capabilities: [u8; 8],
    /// capability 数量。
    pub capability_count: usize,
}

impl DeviceGrant {
    /// 判断是否为 virtio 设备。
    pub fn is_virtio(&self) -> bool {
        self.capabilities[..self.capability_count.min(self.capabilities.len())].contains(&0x09)
    }
}

/// 序列化后的固定长度。
pub const DEVICE_GRANT_BYTES: usize = 10 + 6 * 17 + 6 * 5 + 17 + 2 + 9;

impl DeviceGrant {
    /// 编码为固定布局字节串。
    pub fn encode(&self, buffer: &mut [u8]) -> Option<usize> {
        if buffer.len() < DEVICE_GRANT_BYTES {
            return None;
        }
        let mut at = 0;
        put_u16(buffer, &mut at, self.vendor);
        put_u16(buffer, &mut at, self.device);
        put_u16(buffer, &mut at, self.address.segment);
        put_u8(buffer, &mut at, self.address.bus);
        put_u8(buffer, &mut at, self.address.device);
        put_u8(buffer, &mut at, self.address.function);
        put_u8(buffer, &mut at, self.address.offset);
        for range in self.mmio {
            put_u8(buffer, &mut at, u8::from(range.is_some()));
            if let Some(range) = range {
                put_u64(buffer, &mut at, range.base);
                put_u64(buffer, &mut at, range.len);
            } else {
                put_u64(buffer, &mut at, 0);
                put_u64(buffer, &mut at, 0);
            }
        }
        for range in self.io {
            put_u8(buffer, &mut at, u8::from(range.is_some()));
            let (base, len) = range.map_or((0, 0), |range| (range.base, range.len));
            put_u16(buffer, &mut at, base);
            put_u16(buffer, &mut at, len);
        }
        put_u8(buffer, &mut at, u8::from(self.dma.is_some()));
        let (dma_base, dma_len) = self
            .dma
            .map_or((0, 0), |range| (range.start().get(), range.len()));
        put_u64(buffer, &mut at, dma_base);
        put_u64(buffer, &mut at, dma_len);
        put_u8(buffer, &mut at, u8::from(self.irq.is_some()));
        put_u8(buffer, &mut at, self.irq.unwrap_or(0));
        put_u8(buffer, &mut at, self.capability_count as u8);
        for id in self.capabilities {
            put_u8(buffer, &mut at, id);
        }
        Some(at)
    }

    /// 从固定布局字节串解码。
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < DEVICE_GRANT_BYTES {
            return None;
        }
        let mut at = 0;
        let vendor = get_u16(bytes, &mut at)?;
        let device = get_u16(bytes, &mut at)?;
        let segment = get_u16(bytes, &mut at)?;
        let bus = get_u8(bytes, &mut at)?;
        let device_number = get_u8(bytes, &mut at)?;
        let function = get_u8(bytes, &mut at)?;
        let offset = get_u8(bytes, &mut at)?;
        let mut mmio = [None; 6];
        for slot in &mut mmio {
            let present = get_u8(bytes, &mut at)?;
            let base = get_u64(bytes, &mut at)?;
            let len = get_u64(bytes, &mut at)?;
            if present != 0 {
                *slot = Some(MmioRange { base, len });
            }
        }
        let mut io = [None; 6];
        for slot in &mut io {
            let present = get_u8(bytes, &mut at)?;
            let base = get_u16(bytes, &mut at)?;
            let len = get_u16(bytes, &mut at)?;
            if present != 0 {
                *slot = Some(IoRange { base, len });
            }
        }
        let dma = (get_u8(bytes, &mut at)? != 0).then(|| {
            let base = get_u64(bytes, &mut at).unwrap_or(0);
            let len = get_u64(bytes, &mut at).unwrap_or(0);
            PhysRange::new(PhysAddr::new(base), len)
        });
        if dma.is_none() {
            let _ = get_u64(bytes, &mut at)?;
            let _ = get_u64(bytes, &mut at)?;
        }
        let irq = (get_u8(bytes, &mut at)? != 0).then(|| get_u8(bytes, &mut at).unwrap_or(0));
        if irq.is_none() {
            let _ = get_u8(bytes, &mut at)?;
        }
        let capability_count = usize::from(get_u8(bytes, &mut at)?);
        if capability_count > 8 {
            return None;
        }
        let mut capabilities = [0_u8; 8];
        for id in &mut capabilities {
            *id = get_u8(bytes, &mut at)?;
        }
        Some(Self {
            address: PciAddress {
                segment,
                bus,
                device: device_number,
                function,
                offset,
            },
            vendor,
            device,
            mmio,
            io,
            dma,
            irq,
            capabilities,
            capability_count,
        })
    }
}

fn put_u8(buffer: &mut [u8], at: &mut usize, value: u8) {
    buffer[*at] = value;
    *at += 1;
}

fn put_u16(buffer: &mut [u8], at: &mut usize, value: u16) {
    buffer[*at..*at + 2].copy_from_slice(&value.to_le_bytes());
    *at += 2;
}

fn put_u64(buffer: &mut [u8], at: &mut usize, value: u64) {
    buffer[*at..*at + 8].copy_from_slice(&value.to_le_bytes());
    *at += 8;
}

fn get_u8(bytes: &[u8], at: &mut usize) -> Option<u8> {
    let value = *bytes.get(*at)?;
    *at += 1;
    Some(value)
}

fn get_u16(bytes: &[u8], at: &mut usize) -> Option<u16> {
    let value = u16::from_le_bytes(bytes.get(*at..*at + 2)?.try_into().ok()?);
    *at += 2;
    Some(value)
}

fn get_u64(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let value = u64::from_le_bytes(bytes.get(*at..*at + 8)?.try_into().ok()?);
    *at += 8;
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_device_grant() {
        let grant = DeviceGrant {
            address: PciAddress::new(0, 1, 0, 0),
            vendor: 0x1AF4,
            device: 0x1001,
            mmio: [
                Some(MmioRange {
                    base: 0xFE00_0000,
                    len: 0x1000,
                }),
                None,
                None,
                None,
                None,
                None,
            ],
            io: [
                None,
                None,
                Some(IoRange {
                    base: 0xC000,
                    len: 0x20,
                }),
                None,
                None,
                None,
            ],
            dma: Some(PhysRange::new(PhysAddr::new(0x200_0000), 0x1_0000)),
            irq: Some(11),
            capabilities: [0x09, 0x11, 0, 0, 0, 0, 0, 0],
            capability_count: 2,
        };
        let mut buffer = [0_u8; DEVICE_GRANT_BYTES];
        let length = grant.encode(&mut buffer).expect("编码");
        assert_eq!(length, DEVICE_GRANT_BYTES);
        assert_eq!(DeviceGrant::decode(&buffer[..length]), Some(grant));
    }
}
