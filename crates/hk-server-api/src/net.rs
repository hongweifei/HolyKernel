//! 网络服务协议：驱动信息、帧收发与轮询标签。

/// 查询接口信息。
pub const NET_INFO_REQUEST: u64 = 0x700;
/// 接口信息响应。
pub const NET_INFO_RESPONSE: u64 = 0x701;
/// 请求网络驱动退出。
pub const NET_SHUTDOWN: u64 = 0x702;
/// 发送以太网帧。
pub const NET_TX: u64 = 0x703;
/// 接收以太网帧。
pub const NET_RX: u64 = 0x704;
/// 轮询接收队列。
pub const NET_POLL: u64 = 0x705;
/// 驱动完成一次轮询。
pub const NET_POLL_DONE: u64 = 0x706;
/// 查询接口、Socket 与收发统计。
pub const NET_STATUS_REQUEST: u64 = 0x707;
/// 状态响应。
pub const NET_STATUS_RESPONSE: u64 = 0x708;
/// 查询 ARP 邻居表。
pub const NET_ARP_REQUEST: u64 = 0x709;
/// ARP 邻居表响应。
pub const NET_ARP_RESPONSE: u64 = 0x70a;

/// IRQ Notification 位：设备中断。
pub const NET_NOTIFY_IRQ: u64 = 1 << 0;
/// IRQ Notification 位：Network Server 发来 TX 帧。
pub const NET_NOTIFY_TX: u64 = 1 << 1;

/// 以太网帧最大长度。
pub const NET_FRAME_MAX: usize = 1514;
/// 接口信息编码长度：MAC + MTU + link。
pub const NET_INFO_BYTES: usize = 6 + 2 + 1;

/// 状态编码长度（小端定长布局）。
pub const NET_STATUS_BYTES: usize = 50;

const STATUS_IPV4: usize = 6;
const STATUS_PREFIX: usize = 10;
const STATUS_FLAGS: usize = 11;
const STATUS_GATEWAY: usize = 12;
const STATUS_SOCKETS: usize = 16;
const STATUS_RX_FRAMES: usize = 18;
const STATUS_TX_FRAMES: usize = 26;
const STATUS_RX_BYTES: usize = 34;
const STATUS_TX_BYTES: usize = 42;

/// 接口、Socket 与收发统计快照。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetStatus {
    /// 以太网地址。
    pub mac: [u8; 6],
    /// IPv4 地址；`has_ipv4` 为假时无意义。
    pub ipv4: [u8; 4],
    /// 是否配置了 IPv4 地址。
    pub has_ipv4: bool,
    /// IPv4 前缀长度。
    pub prefix: u8,
    /// 默认网关；`has_gateway` 为假时无意义。
    pub gateway: [u8; 4],
    /// 是否存在默认网关。
    pub has_gateway: bool,
    /// 打开的 socket 数量。
    pub sockets: u16,
    /// 接收帧数。
    pub rx_frames: u64,
    /// 发送帧数。
    pub tx_frames: u64,
    /// 接收字节数。
    pub rx_bytes: u64,
    /// 发送字节数。
    pub tx_bytes: u64,
}

impl NetStatus {
    /// 编码为定长字节串，返回写入长度。
    pub fn encode(&self, buffer: &mut [u8]) -> Option<usize> {
        if buffer.len() < NET_STATUS_BYTES {
            return None;
        }
        buffer[..NET_STATUS_BYTES].fill(0);
        buffer[..6].copy_from_slice(&self.mac);
        buffer[STATUS_IPV4..STATUS_IPV4 + 4].copy_from_slice(&self.ipv4);
        buffer[STATUS_PREFIX] = self.prefix;
        let mut flags = 0_u8;
        if self.has_ipv4 {
            flags |= 1 << 0;
        }
        if self.has_gateway {
            flags |= 1 << 1;
        }
        buffer[STATUS_FLAGS] = flags;
        buffer[STATUS_GATEWAY..STATUS_GATEWAY + 4].copy_from_slice(&self.gateway);
        buffer[STATUS_SOCKETS..STATUS_SOCKETS + 2].copy_from_slice(&self.sockets.to_le_bytes());
        buffer[STATUS_RX_FRAMES..STATUS_RX_FRAMES + 8]
            .copy_from_slice(&self.rx_frames.to_le_bytes());
        buffer[STATUS_TX_FRAMES..STATUS_TX_FRAMES + 8]
            .copy_from_slice(&self.tx_frames.to_le_bytes());
        buffer[STATUS_RX_BYTES..STATUS_RX_BYTES + 8].copy_from_slice(&self.rx_bytes.to_le_bytes());
        buffer[STATUS_TX_BYTES..STATUS_TX_BYTES + 8].copy_from_slice(&self.tx_bytes.to_le_bytes());
        Some(NET_STATUS_BYTES)
    }

    /// 从定长字节串解码。
    pub fn decode(buffer: &[u8]) -> Option<Self> {
        if buffer.len() < NET_STATUS_BYTES {
            return None;
        }
        let flags = buffer[STATUS_FLAGS];
        Some(Self {
            mac: buffer[..6].try_into().ok()?,
            ipv4: buffer[STATUS_IPV4..STATUS_IPV4 + 4].try_into().ok()?,
            has_ipv4: flags & 1 != 0,
            prefix: buffer[STATUS_PREFIX],
            gateway: buffer[STATUS_GATEWAY..STATUS_GATEWAY + 4].try_into().ok()?,
            has_gateway: flags & 2 != 0,
            sockets: u16::from_le_bytes(
                buffer[STATUS_SOCKETS..STATUS_SOCKETS + 2].try_into().ok()?,
            ),
            rx_frames: u64::from_le_bytes(
                buffer[STATUS_RX_FRAMES..STATUS_RX_FRAMES + 8]
                    .try_into()
                    .ok()?,
            ),
            tx_frames: u64::from_le_bytes(
                buffer[STATUS_TX_FRAMES..STATUS_TX_FRAMES + 8]
                    .try_into()
                    .ok()?,
            ),
            rx_bytes: u64::from_le_bytes(
                buffer[STATUS_RX_BYTES..STATUS_RX_BYTES + 8]
                    .try_into()
                    .ok()?,
            ),
            tx_bytes: u64::from_le_bytes(
                buffer[STATUS_TX_BYTES..STATUS_TX_BYTES + 8]
                    .try_into()
                    .ok()?,
            ),
        })
    }
}

/// ARP 表最大条目数。
pub const NET_ARP_MAX: usize = 8;
/// 单条 ARP 记录编码长度：IPv4 + MAC。
pub const NET_ARP_ENTRY_BYTES: usize = 10;
/// ARP 表编码长度：计数 + 定长条目。
pub const NET_ARP_BYTES: usize = 1 + NET_ARP_MAX * NET_ARP_ENTRY_BYTES;

/// 一条 ARP 邻居记录。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ArpRecord {
    /// IPv4 地址。
    pub ip: [u8; 4],
    /// 以太网地址。
    pub mac: [u8; 6],
}

/// ARP 邻居表快照。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArpTable {
    /// 有效条目数。
    pub count: u8,
    /// 条目。
    pub entries: [ArpRecord; NET_ARP_MAX],
}

impl ArpTable {
    /// 空表。
    pub const EMPTY: Self = Self {
        count: 0,
        entries: [ArpRecord {
            ip: [0; 4],
            mac: [0; 6],
        }; NET_ARP_MAX],
    };

    /// 编码为定长字节串，返回写入长度。
    pub fn encode(&self, buffer: &mut [u8]) -> Option<usize> {
        if buffer.len() < NET_ARP_BYTES {
            return None;
        }
        buffer[..NET_ARP_BYTES].fill(0);
        let count = usize::from(self.count).min(NET_ARP_MAX);
        buffer[0] = count as u8;
        for (index, entry) in self.entries[..count].iter().enumerate() {
            let offset = 1 + index * NET_ARP_ENTRY_BYTES;
            buffer[offset..offset + 4].copy_from_slice(&entry.ip);
            buffer[offset + 4..offset + NET_ARP_ENTRY_BYTES].copy_from_slice(&entry.mac);
        }
        Some(NET_ARP_BYTES)
    }

    /// 从定长字节串解码。
    pub fn decode(buffer: &[u8]) -> Option<Self> {
        let count = usize::from(*buffer.first()?);
        if count > NET_ARP_MAX || buffer.len() < 1 + count * NET_ARP_ENTRY_BYTES {
            return None;
        }
        let mut table = Self::EMPTY;
        table.count = count as u8;
        for (index, entry) in table.entries[..count].iter_mut().enumerate() {
            let offset = 1 + index * NET_ARP_ENTRY_BYTES;
            entry.ip.copy_from_slice(&buffer[offset..offset + 4]);
            entry
                .mac
                .copy_from_slice(&buffer[offset + 4..offset + NET_ARP_ENTRY_BYTES]);
        }
        Some(table)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_status() {
        let status = NetStatus {
            mac: [1, 2, 3, 4, 5, 6],
            ipv4: [10, 0, 2, 15],
            has_ipv4: true,
            prefix: 24,
            gateway: [10, 0, 2, 2],
            has_gateway: true,
            sockets: 3,
            rx_frames: 11,
            tx_frames: 12,
            rx_bytes: 13,
            tx_bytes: 14,
        };
        let mut buffer = [0_u8; NET_STATUS_BYTES];
        assert_eq!(status.encode(&mut buffer), Some(NET_STATUS_BYTES));
        assert_eq!(NetStatus::decode(&buffer), Some(status));
        assert_eq!(NetStatus::decode(&buffer[..NET_STATUS_BYTES - 1]), None);
    }

    #[test]
    fn round_trips_arp_table() {
        let mut table = ArpTable::EMPTY;
        table.count = 2;
        table.entries[0] = ArpRecord {
            ip: [10, 0, 2, 2],
            mac: [0x52, 0x55, 0x0a, 0x00, 0x02, 0x02],
        };
        table.entries[1] = ArpRecord {
            ip: [10, 0, 2, 3],
            mac: [0x52, 0x55, 0x0a, 0x00, 0x02, 0x03],
        };
        let mut buffer = [0_u8; NET_ARP_BYTES];
        assert_eq!(table.encode(&mut buffer), Some(NET_ARP_BYTES));
        assert_eq!(ArpTable::decode(&buffer), Some(table));
        assert_eq!(ArpTable::decode(&[0_u8]), Some(ArpTable::EMPTY));
        assert_eq!(
            ArpTable::decode(&buffer[..1 + 2 * NET_ARP_ENTRY_BYTES - 1]),
            None
        );
    }
}
