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
/// 查询原生网络配置。
pub const NET_CONFIG_REQUEST: u64 = 0x70b;
/// 原生网络配置响应。
pub const NET_CONFIG_RESPONSE: u64 = 0x70c;
/// 驱动向 Network Server 移交共享帧（`word` 打包角色与索引）。
pub const NET_MAPPING_ATTACH: u64 = 0x70d;
/// 驱动通知一帧接收数据已就绪（`word` 打包索引、偏移与长度）。
pub const NET_RX_FRAME: u64 = 0x70e;
/// Network Server 归还接收帧（`word` 为索引）。
pub const NET_RX_REFILL: u64 = 0x70f;
/// Network Server 提交发送帧（`word` 打包索引与长度）。
pub const NET_TX_FRAME: u64 = 0x710;
/// 驱动通知发送帧已完成（`word` 为索引）。
pub const NET_TX_DONE: u64 = 0x711;

/// 共享帧角色：接收。
pub const NET_MAPPING_ROLE_RX: u64 = 0;
/// 共享帧角色：发送。
pub const NET_MAPPING_ROLE_TX: u64 = 1;
/// 共享接收帧数量。
pub const NET_SHARED_RX: usize = 8;
/// 共享发送帧数量。
pub const NET_SHARED_TX: usize = 8;
/// 单个共享帧长度。
pub const NET_SHARED_LEN: usize = 4096;
/// 发送帧中为 virtio 头预留的空间。
pub const NET_TX_HEADER: usize = 32;

/// 打包共享帧角色与索引。
pub const fn net_mapping_word(role: u64, index: usize) -> u64 {
    (role << 8) | index as u64
}

/// 从共享帧消息中取角色。
pub const fn net_mapping_role(word: u64) -> u64 {
    word >> 8
}

/// 从共享帧消息中取索引。
pub const fn net_mapping_index(word: u64) -> usize {
    (word & 0xFF) as usize
}

/// 打包接收帧索引、偏移与长度。
pub const fn net_rx_word(index: usize, offset: usize, length: usize) -> u64 {
    (index as u64 & 0xFF) | ((offset as u64 & 0xFF) << 16) | ((length as u64 & 0xFFFF) << 32)
}

/// 从接收帧消息中取索引。
pub const fn net_rx_index(word: u64) -> usize {
    (word & 0xFF) as usize
}

/// 从接收帧消息中取偏移。
pub const fn net_rx_offset(word: u64) -> usize {
    ((word >> 16) & 0xFF) as usize
}

/// 从接收帧消息中取长度。
pub const fn net_rx_length(word: u64) -> usize {
    ((word >> 32) & 0xFFFF) as usize
}

/// 打包发送帧索引与长度。
pub const fn net_tx_word(index: usize, length: usize) -> u64 {
    (index as u64 & 0xFF) | ((length as u64 & 0xFFFF) << 32)
}

/// 从发送帧消息中取索引。
pub const fn net_tx_index(word: u64) -> usize {
    (word & 0xFF) as usize
}

/// 从发送帧消息中取长度。
pub const fn net_tx_length(word: u64) -> usize {
    ((word >> 32) & 0xFFFF) as usize
}

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

/// 配置中最多携带的 DNS 服务器数量。
pub const NET_CONFIG_MAX_DNS: usize = 4;
/// 配置编码长度：标志 + 地址/网关 + MTU + DNS 列表。
pub const NET_CONFIG_BYTES: usize = 13 + NET_CONFIG_MAX_DNS * 4;

const CONFIG_FLAGS: usize = 0;
const CONFIG_IPV4: usize = 1;
const CONFIG_PREFIX: usize = 5;
const CONFIG_GATEWAY: usize = 6;
const CONFIG_MTU: usize = 10;
const CONFIG_DNS_COUNT: usize = 12;
const CONFIG_DNS: usize = 13;

/// 原生网络配置快照。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetConfig {
    /// 是否通过 DHCP 获得配置。
    pub dhcp: bool,
    /// IPv4 地址。
    pub ipv4: [u8; 4],
    /// 是否配置了 IPv4 地址。
    pub has_ipv4: bool,
    /// 前缀长度。
    pub prefix: u8,
    /// 默认网关。
    pub gateway: [u8; 4],
    /// 是否存在默认网关。
    pub has_gateway: bool,
    /// MTU。
    pub mtu: u16,
    /// DNS 服务器数量。
    pub dns_count: u8,
    /// DNS 服务器。
    pub dns: [[u8; 4]; NET_CONFIG_MAX_DNS],
}

impl NetConfig {
    /// 编码为定长字节串，返回写入长度。
    pub fn encode(&self, buffer: &mut [u8]) -> Option<usize> {
        if buffer.len() < NET_CONFIG_BYTES {
            return None;
        }
        buffer[..NET_CONFIG_BYTES].fill(0);
        let mut flags = 0_u8;
        if self.dhcp {
            flags |= 1 << 0;
        }
        if self.has_ipv4 {
            flags |= 1 << 1;
        }
        if self.has_gateway {
            flags |= 1 << 2;
        }
        buffer[CONFIG_FLAGS] = flags;
        buffer[CONFIG_IPV4..CONFIG_IPV4 + 4].copy_from_slice(&self.ipv4);
        buffer[CONFIG_PREFIX] = self.prefix;
        buffer[CONFIG_GATEWAY..CONFIG_GATEWAY + 4].copy_from_slice(&self.gateway);
        buffer[CONFIG_MTU..CONFIG_MTU + 2].copy_from_slice(&self.mtu.to_le_bytes());
        let count = usize::from(self.dns_count).min(NET_CONFIG_MAX_DNS);
        buffer[CONFIG_DNS_COUNT] = count as u8;
        for (index, server) in self.dns[..count].iter().enumerate() {
            let offset = CONFIG_DNS + index * 4;
            buffer[offset..offset + 4].copy_from_slice(server);
        }
        Some(NET_CONFIG_BYTES)
    }

    /// 从定长字节串解码。
    pub fn decode(buffer: &[u8]) -> Option<Self> {
        if buffer.len() < NET_CONFIG_BYTES {
            return None;
        }
        let flags = buffer[CONFIG_FLAGS];
        let count = usize::from(buffer[CONFIG_DNS_COUNT]);
        if count > NET_CONFIG_MAX_DNS {
            return None;
        }
        let mut dns = [[0_u8; 4]; NET_CONFIG_MAX_DNS];
        for (index, server) in dns[..count].iter_mut().enumerate() {
            let offset = CONFIG_DNS + index * 4;
            server.copy_from_slice(&buffer[offset..offset + 4]);
        }
        Some(Self {
            dhcp: flags & 1 != 0,
            ipv4: buffer[CONFIG_IPV4..CONFIG_IPV4 + 4].try_into().ok()?,
            has_ipv4: flags & 2 != 0,
            prefix: buffer[CONFIG_PREFIX],
            gateway: buffer[CONFIG_GATEWAY..CONFIG_GATEWAY + 4].try_into().ok()?,
            has_gateway: flags & 4 != 0,
            mtu: u16::from_le_bytes(buffer[CONFIG_MTU..CONFIG_MTU + 2].try_into().ok()?),
            dns_count: count as u8,
            dns,
        })
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

    #[test]
    fn round_trips_config() {
        let config = NetConfig {
            dhcp: true,
            ipv4: [10, 0, 2, 15],
            has_ipv4: true,
            prefix: 24,
            gateway: [10, 0, 2, 2],
            has_gateway: true,
            mtu: 1500,
            dns_count: 2,
            dns: [[10, 0, 2, 3], [10, 0, 2, 4], [0; 4], [0; 4]],
        };
        let mut buffer = [0_u8; NET_CONFIG_BYTES];
        assert_eq!(config.encode(&mut buffer), Some(NET_CONFIG_BYTES));
        assert_eq!(NetConfig::decode(&buffer), Some(config));
        assert_eq!(NetConfig::decode(&buffer[..NET_CONFIG_BYTES - 1]), None);
    }
}
