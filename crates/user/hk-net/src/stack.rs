//! smoltcp 接口、SocketSet 与原生 UDP 包装。

use alloc::vec;
use alloc::vec::Vec;

use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet, SocketStorage};
use smoltcp::socket::{dhcpv4, dns, icmp, tcp, udp};
use smoltcp::storage::PacketMetadata;
use smoltcp::time::Instant;
use smoltcp::wire::{
    DnsQueryType, EthernetAddress, HardwareAddress, Icmpv4Message, Icmpv4Packet, IpAddress, IpCidr,
    IpEndpoint, Ipv4Address,
};

use crate::error::NetError;
use crate::phy::FrameQueue;

const UDP_RX_SLOTS: usize = 8;
const UDP_TX_SLOTS: usize = 8;
const UDP_BUFFER: usize = 2048;
/// ARP 邻居缓存容量。
const ARP_CACHE_SIZE: usize = 8;
/// ARP 帧的以太网类型。
const ETHERTYPE_ARP: [u8; 2] = [0x08, 0x06];

/// DHCP 租约。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DhcpLease {
    /// 分配的 IPv4 地址。
    pub address: Ipv4Address,
    /// 前缀长度。
    pub prefix: u8,
    /// 默认网关。
    pub gateway: Option<Ipv4Address>,
    /// DNS 服务器。
    pub dns: Vec<Ipv4Address>,
}

/// 接口状态快照。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StackStatus {
    /// 以太网地址。
    pub mac: [u8; 6],
    /// IPv4 地址；未配置时为空。
    pub ipv4: Option<Ipv4Address>,
    /// IPv4 前缀长度。
    pub prefix: u8,
    /// 默认网关。
    pub gateway: Option<Ipv4Address>,
    /// 当前打开的 socket 数量。
    pub sockets: usize,
}

/// 观察到的 ARP 邻居。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArpEntry {
    /// IPv4 地址。
    pub ip: Ipv4Address,
    /// 对应的以太网地址。
    pub mac: EthernetAddress,
}

impl ArpEntry {
    const EMPTY: Self = Self {
        ip: Ipv4Address::new(0, 0, 0, 0),
        mac: EthernetAddress([0; 6]),
    };
}

/// 对外 Socket 句柄（由协议栈分配）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SocketId(pub usize);

enum Entry {
    Tcp(SocketHandle),
    Udp(SocketHandle),
}

/// 一个以太网接口上的 smoltcp 协议栈。
pub struct Stack {
    device: FrameQueue,
    interface: Interface,
    sockets: SocketSet<'static>,
    entries: Vec<Option<Entry>>,
    arp: [ArpEntry; ARP_CACHE_SIZE],
    arp_len: usize,
    arp_next: usize,
}

impl Stack {
    /// 构造静态 IPv4 接口；`prefix` 为前缀长度。
    pub fn new(
        mac: [u8; 6],
        address: Ipv4Address,
        prefix: u8,
        gateway: Option<Ipv4Address>,
        random_seed: u64,
    ) -> Result<Self, NetError> {
        if prefix > 32 {
            return Err(NetError::Address);
        }
        let mut device = FrameQueue::new();
        let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
        config.random_seed = random_seed;
        let mut interface = Interface::new(config, &mut device, Instant::from_millis(0));
        interface.update_ip_addrs(|addresses| {
            let _ = addresses.push(IpCidr::new(IpAddress::Ipv4(address), prefix));
        });
        if let Some(gateway) = gateway {
            interface
                .routes_mut()
                .add_default_ipv4_route(gateway)
                .map_err(|_| NetError::Route)?;
        }
        let sockets: SocketSet<'static> = SocketSet::new(Vec::<SocketStorage<'static>>::new());
        Ok(Self {
            device,
            interface,
            sockets,
            entries: Vec::new(),
            arp: [ArpEntry::EMPTY; ARP_CACHE_SIZE],
            arp_len: 0,
            arp_next: 0,
        })
    }

    /// 构造等待 DHCP 租约的接口，返回协议栈与 DHCP SocketHandle。
    pub fn new_dhcp(mac: [u8; 6], random_seed: u64) -> Result<(Self, SocketHandle), NetError> {
        let mut device = FrameQueue::new();
        let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
        config.random_seed = random_seed;
        let interface = Interface::new(config, &mut device, Instant::from_millis(0));
        let mut sockets: SocketSet<'static> = SocketSet::new(Vec::<SocketStorage<'static>>::new());
        let handle = sockets.add(dhcpv4::Socket::new());
        Ok((
            Self {
                device,
                interface,
                sockets,
                entries: Vec::new(),
                arp: [ArpEntry::EMPTY; ARP_CACHE_SIZE],
                arp_len: 0,
                arp_next: 0,
            },
            handle,
        ))
    }

    /// 应用静态 IPv4 配置。
    pub fn set_ipv4(
        &mut self,
        address: Ipv4Address,
        prefix: u8,
        gateway: Option<Ipv4Address>,
    ) -> Result<(), NetError> {
        if prefix > 32 {
            return Err(NetError::Address);
        }
        self.interface.update_ip_addrs(|addresses| {
            addresses.clear();
            let _ = addresses.push(IpCidr::new(IpAddress::Ipv4(address), prefix));
        });
        if let Some(gateway) = gateway {
            self.interface
                .routes_mut()
                .add_default_ipv4_route(gateway)
                .map_err(|_| NetError::Route)?;
        }
        Ok(())
    }

    /// 读取 DHCP 事件；获得租约时应用到接口并返回租约。
    pub fn poll_dhcp(&mut self, handle: SocketHandle) -> Option<DhcpLease> {
        let event = self.sockets.get_mut::<dhcpv4::Socket>(handle).poll()?;
        match event {
            dhcpv4::Event::Configured(config) => {
                let lease = DhcpLease {
                    address: config.address.address(),
                    prefix: config.address.prefix_len(),
                    gateway: config.router,
                    dns: config.dns_servers.iter().copied().collect(),
                };
                let _ = self.set_ipv4(lease.address, lease.prefix, lease.gateway);
                Some(lease)
            }
            dhcpv4::Event::Deconfigured => {
                self.interface
                    .update_ip_addrs(|addresses| addresses.clear());
                None
            }
        }
    }

    /// 返回接口状态快照。
    pub fn status(&self) -> StackStatus {
        let HardwareAddress::Ethernet(address) = self.interface.hardware_addr();
        let mac = address.0;
        let mut ipv4 = None;
        let mut prefix = 0;
        for address in self.interface.ip_addrs() {
            if let IpCidr::Ipv4(cidr) = address {
                ipv4 = Some(cidr.address());
                prefix = cidr.prefix_len();
                break;
            }
        }
        let gateway =
            self.interface
                .routes()
                .get_default_ipv4_route()
                .and_then(|route| match route.via_router {
                    IpAddress::Ipv4(address) => Some(address),
                    _ => None,
                });
        StackStatus {
            mac,
            ipv4,
            prefix,
            gateway,
            sockets: self.sockets.iter().count(),
        }
    }

    /// 返回观察到的 ARP 邻居。
    pub fn arp_entries(&self) -> &[ArpEntry] {
        &self.arp[..self.arp_len]
    }

    /// 启用共享帧环。
    pub fn enable_shared(&mut self, rx: Vec<u64>, tx: Vec<u64>, frame_len: usize) {
        self.device.enable_shared(rx, tx, frame_len);
    }

    /// 注入一个共享接收帧；顺带观察 ARP 以便邻居视图继续可用。
    pub fn push_shared_rx(&mut self, index: usize, offset: usize, length: usize) {
        let mut head = [0_u8; 42];
        if let Some(bytes) = self.device.shared_slice(index, offset, length) {
            let count = bytes.len().min(head.len());
            head[..count].copy_from_slice(&bytes[..count]);
            self.observe_arp(&head[..count]);
        }
        self.device.push_shared_rx(index, offset, length);
    }

    /// 取走协议栈已消费的接收帧与待发送帧。
    pub fn take_shared_events(&mut self, rx: &mut Vec<usize>, tx: &mut Vec<(usize, usize)>) {
        self.device.take_shared_events(rx, tx);
    }

    /// 归还发送完成的共享帧。
    pub fn shared_tx_done(&mut self, index: usize) {
        self.device.shared_tx_done(index);
    }

    /// 注入一帧接收数据。
    pub fn push_frame(&mut self, frame: &[u8]) -> Result<(), NetError> {
        self.observe_arp(frame);
        if self.device.push_rx(frame) {
            Ok(())
        } else {
            Err(NetError::Queue)
        }
    }

    /// 从入站 ARP 帧记录邻居。smoltcp 的邻居缓存不对外公开，这里旁路观察。
    fn observe_arp(&mut self, frame: &[u8]) {
        const ARP_LEN: usize = 28;
        if frame.len() < 14 + ARP_LEN || frame[12..14] != ETHERTYPE_ARP {
            return;
        }
        let arp = &frame[14..14 + ARP_LEN];
        if arp[0..2] != [0, 1] || arp[2..4] != [0x08, 0x00] || arp[4] != 6 || arp[5] != 4 {
            return;
        }
        let operation = u16::from_be_bytes([arp[6], arp[7]]);
        if operation != 1 && operation != 2 {
            return;
        }
        let mut mac = [0_u8; 6];
        mac.copy_from_slice(&arp[8..14]);
        let mut ip = [0_u8; 4];
        ip.copy_from_slice(&arp[14..18]);
        if ip == [0; 4] || mac == [0; 6] {
            return;
        }
        self.remember_arp(Ipv4Address::from(ip), EthernetAddress(mac));
    }

    fn remember_arp(&mut self, ip: Ipv4Address, mac: EthernetAddress) {
        if let Some(entry) = self.arp[..self.arp_len]
            .iter_mut()
            .find(|entry| entry.ip == ip)
        {
            entry.mac = mac;
            return;
        }
        let entry = ArpEntry { ip, mac };
        if self.arp_len < ARP_CACHE_SIZE {
            self.arp[self.arp_len] = entry;
            self.arp_len += 1;
        } else {
            self.arp[self.arp_next] = entry;
            self.arp_next = (self.arp_next + 1) % ARP_CACHE_SIZE;
        }
    }

    /// 取出一帧待发送数据。
    pub fn take_cow_page(&mut self) -> Option<Vec<u8>> {
        self.device.pop_tx()
    }

    /// 推进协议栈。
    pub fn poll(&mut self, milliseconds: u64) {
        self.interface.poll(
            Instant::from_millis(milliseconds as i64),
            &mut self.device,
            &mut self.sockets,
        );
    }

    /// 绑定一个 UDP 端口，返回 SocketHandle。
    pub fn open_udp(&mut self, port: u16) -> Result<SocketHandle, NetError> {
        let rx = udp::PacketBuffer::new(
            vec![PacketMetadata::EMPTY; UDP_RX_SLOTS],
            vec![0_u8; UDP_BUFFER],
        );
        let tx = udp::PacketBuffer::new(
            vec![PacketMetadata::EMPTY; UDP_TX_SLOTS],
            vec![0_u8; UDP_BUFFER],
        );
        let mut socket = udp::Socket::new(rx, tx);
        socket.bind(port).map_err(|_| NetError::Bind)?;
        Ok(self.sockets.add(socket))
    }

    /// 打开 DNS socket，配置服务器列表。
    pub fn open_dns(&mut self, servers: &[Ipv4Address]) -> Result<SocketHandle, NetError> {
        if servers.is_empty() {
            return Err(NetError::Address);
        }
        let addresses: Vec<IpAddress> = servers.iter().copied().map(IpAddress::Ipv4).collect();
        let socket = dns::Socket::new(&addresses, Vec::new());
        Ok(self.sockets.add(socket))
    }

    /// 关闭 socket，释放查询槽与缓冲区。
    pub fn close_socket(&mut self, handle: SocketHandle) {
        self.sockets.remove(handle);
    }

    /// 打开一个对外 TCP socket。
    pub fn socket_open_tcp(&mut self, buffer: usize) -> Option<SocketId> {
        let handle = self.open_tcp(buffer).ok()?;
        self.entries.push(Some(Entry::Tcp(handle)));
        Some(SocketId(self.entries.len() - 1))
    }

    /// 打开一个对外 UDP socket 并绑定本地端口。
    pub fn socket_open_udp(&mut self, port: u16) -> Option<SocketId> {
        let handle = self.open_udp(port).ok()?;
        self.entries.push(Some(Entry::Udp(handle)));
        Some(SocketId(self.entries.len() - 1))
    }

    /// TCP 主动连接。
    pub fn socket_connect(
        &mut self,
        id: SocketId,
        remote: Ipv4Address,
        port: u16,
        local_port: u16,
    ) -> bool {
        match self.entries.get(id.0) {
            Some(Some(Entry::Tcp(handle))) => {
                self.connect_tcp(*handle, remote, port, local_port).is_ok()
            }
            _ => false,
        }
    }

    /// 发送 TCP 数据，返回写入长度。
    pub fn socket_send(&mut self, id: SocketId, data: &[u8]) -> Option<usize> {
        match self.entries.get(id.0) {
            Some(Some(Entry::Tcp(handle))) => self.send_tcp(*handle, data).ok(),
            _ => None,
        }
    }

    /// 接收 TCP 数据；无数据返回 `Some(None)`。
    pub fn socket_recv(
        &mut self,
        id: SocketId,
        buffer: &mut [u8],
    ) -> Result<Option<usize>, NetError> {
        match self.entries.get(id.0) {
            Some(Some(Entry::Tcp(handle))) => self.recv_tcp(*handle, buffer),
            _ => Err(NetError::Recv),
        }
    }

    /// UDP 发送到指定远端。
    pub fn socket_udp_send(
        &mut self,
        id: SocketId,
        remote: Ipv4Address,
        port: u16,
        data: &[u8],
    ) -> bool {
        match self.entries.get(id.0) {
            Some(Some(Entry::Udp(handle))) => self.send_udp(*handle, remote, port, data).is_ok(),
            _ => false,
        }
    }

    /// 接收 UDP 数据；无数据返回 `Some(None)`。
    pub fn socket_udp_recv(
        &mut self,
        id: SocketId,
        buffer: &mut [u8],
    ) -> Result<Option<usize>, NetError> {
        match self.entries.get(id.0) {
            Some(Some(Entry::Udp(handle))) => self
                .recv_udp(*handle, buffer)
                .map(|value| value.map(|(length, _)| length)),
            _ => Err(NetError::Recv),
        }
    }

    /// 关闭对外 socket。
    pub fn socket_close(&mut self, id: SocketId) {
        let Some(slot) = self.entries.get_mut(id.0) else {
            return;
        };
        if let Some(entry) = slot.take() {
            match entry {
                Entry::Tcp(handle) | Entry::Udp(handle) => self.close_socket(handle),
            }
        }
    }

    /// 启动一次 DNS 查询；`DnsQueryType::A` 查询 IPv4，`DnsQueryType::Aaaa` 查询 IPv6。
    pub fn start_dns_query(
        &mut self,
        handle: SocketHandle,
        name: &str,
        query_type: DnsQueryType,
    ) -> Result<dns::QueryHandle, NetError> {
        self.sockets
            .get_mut::<dns::Socket>(handle)
            .start_query(self.interface.context(), name, query_type)
            .map_err(|_| NetError::Send)
    }

    /// 读取 DNS 查询结果；未完成时返回 `None`，失败返回 [`NetError::Recv`]。
    ///
    /// 成功后查询槽会被 smoltcp 自动回收，返回的地址同时包含 A 与 AAAA 记录。
    pub fn poll_dns(
        &mut self,
        handle: SocketHandle,
        query: dns::QueryHandle,
    ) -> Result<Option<Vec<IpAddress>>, NetError> {
        match self
            .sockets
            .get_mut::<dns::Socket>(handle)
            .get_query_result(query)
        {
            Ok(addresses) => Ok(Some(addresses.into_iter().collect())),
            Err(dns::GetQueryResultError::Pending) => Ok(None),
            Err(dns::GetQueryResultError::Failed) => Err(NetError::Recv),
        }
    }

    /// 打开一个 TCP socket。
    pub fn open_tcp(&mut self, buffer: usize) -> Result<SocketHandle, NetError> {
        let socket = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0_u8; buffer]),
            tcp::SocketBuffer::new(vec![0_u8; buffer]),
        );
        Ok(self.sockets.add(socket))
    }

    /// 监听本地 TCP 端口。
    pub fn listen_tcp(&mut self, handle: SocketHandle, port: u16) -> Result<(), NetError> {
        self.sockets
            .get_mut::<tcp::Socket>(handle)
            .listen(port)
            .map_err(|_| NetError::Bind)
    }

    /// 主动连接远端 TCP 端点。
    pub fn connect_tcp(
        &mut self,
        handle: SocketHandle,
        remote: Ipv4Address,
        port: u16,
        local_port: u16,
    ) -> Result<(), NetError> {
        let endpoint = IpEndpoint::new(IpAddress::Ipv4(remote), port);
        let socket = self.sockets.get_mut::<tcp::Socket>(handle);
        socket
            .connect(self.interface.context(), endpoint, local_port)
            .map_err(|_| NetError::Send)
    }

    /// TCP 是否已建立或可发送。
    pub fn tcp_may_send(&mut self, handle: SocketHandle) -> bool {
        self.sockets.get_mut::<tcp::Socket>(handle).may_send()
    }

    /// TCP 是否可接收。
    pub fn tcp_may_recv(&mut self, handle: SocketHandle) -> bool {
        self.sockets.get_mut::<tcp::Socket>(handle).may_recv()
    }

    /// 发送 TCP 数据，返回实际入队长度。
    pub fn send_tcp(&mut self, handle: SocketHandle, data: &[u8]) -> Result<usize, NetError> {
        self.sockets
            .get_mut::<tcp::Socket>(handle)
            .send_slice(data)
            .map_err(|_| NetError::Send)
    }

    /// 接收 TCP 数据。
    pub fn recv_tcp(
        &mut self,
        handle: SocketHandle,
        buffer: &mut [u8],
    ) -> Result<Option<usize>, NetError> {
        let socket = self.sockets.get_mut::<tcp::Socket>(handle);
        if !socket.can_recv() {
            return Ok(None);
        }
        socket
            .recv_slice(buffer)
            .map(Some)
            .map_err(|_| NetError::Recv)
    }

    /// 打开 ICMP socket，绑定一个标识。
    pub fn open_icmp(&mut self, ident: u16) -> Result<SocketHandle, NetError> {
        let rx = icmp::PacketBuffer::new(vec![PacketMetadata::EMPTY; 4], vec![0_u8; 1024]);
        let tx = icmp::PacketBuffer::new(vec![PacketMetadata::EMPTY; 4], vec![0_u8; 1024]);
        let mut socket = icmp::Socket::new(rx, tx);
        socket
            .bind(icmp::Endpoint::Ident(ident))
            .map_err(|_| NetError::Bind)?;
        Ok(self.sockets.add(socket))
    }

    /// 发送 ICMP echo request。
    pub fn send_ping(
        &mut self,
        handle: SocketHandle,
        target: Ipv4Address,
        ident: u16,
        sequence: u16,
        payload: &[u8],
    ) -> Result<(), NetError> {
        let socket = self.sockets.get_mut::<icmp::Socket>(handle);
        let size = 8 + payload.len();
        socket
            .send_with(size, IpAddress::Ipv4(target), |buffer| {
                buffer[8..].copy_from_slice(payload);
                let mut packet = Icmpv4Packet::new_unchecked(buffer);
                packet.set_msg_type(Icmpv4Message::EchoRequest);
                packet.set_msg_code(0);
                packet.set_echo_ident(ident);
                packet.set_echo_seq_no(sequence);
                packet.fill_checksum();
                size
            })
            .map_err(|_| NetError::Send)?;
        Ok(())
    }

    /// 接收 ICMP echo reply，返回长度与来源。
    pub fn recv_ping(
        &mut self,
        handle: SocketHandle,
        buffer: &mut [u8],
    ) -> Result<Option<(usize, Ipv4Address)>, NetError> {
        let socket = self.sockets.get_mut::<icmp::Socket>(handle);
        if !socket.can_recv() {
            return Ok(None);
        }
        let (length, address) = socket.recv_slice(buffer).map_err(|_| NetError::Recv)?;
        let IpAddress::Ipv4(address) = address else {
            return Ok(None);
        };
        let packet = Icmpv4Packet::new_checked(&buffer[..length]).map_err(|_| NetError::Recv)?;
        if packet.msg_type() != Icmpv4Message::EchoReply {
            return Ok(None);
        }
        Ok(Some((length, address)))
    }

    /// 发送 UDP 数据。
    pub fn send_udp(
        &mut self,
        handle: SocketHandle,
        remote: Ipv4Address,
        port: u16,
        data: &[u8],
    ) -> Result<(), NetError> {
        let socket = self.sockets.get_mut::<udp::Socket>(handle);
        socket
            .send_slice(data, IpEndpoint::new(IpAddress::Ipv4(remote), port))
            .map_err(|_| NetError::Send)
    }

    /// 接收 UDP 数据，返回长度与远端地址。
    pub fn recv_udp(
        &mut self,
        handle: SocketHandle,
        buffer: &mut [u8],
    ) -> Result<Option<(usize, IpEndpoint)>, NetError> {
        let socket = self.sockets.get_mut::<udp::Socket>(handle);
        if !socket.can_recv() {
            return Ok(None);
        }
        socket
            .recv_slice(buffer)
            .map(|(length, metadata)| Some((length, metadata.endpoint)))
            .map_err(|_| NetError::Recv)
    }
}
