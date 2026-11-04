//! smoltcp 接口、SocketSet 与原生 UDP 包装。

use alloc::vec;
use alloc::vec::Vec;

use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet, SocketStorage};
use smoltcp::socket::{icmp, udp};
use smoltcp::storage::PacketMetadata;
use smoltcp::time::Instant;
use smoltcp::wire::{
    EthernetAddress, HardwareAddress, Icmpv4Message, Icmpv4Packet, IpAddress, IpCidr, IpEndpoint,
    Ipv4Address,
};

use crate::error::NetError;
use crate::phy::FrameQueue;

const UDP_RX_SLOTS: usize = 8;
const UDP_TX_SLOTS: usize = 8;
const UDP_BUFFER: usize = 2048;

/// 一个以太网接口上的 smoltcp 协议栈。
pub struct Stack {
    device: FrameQueue,
    interface: Interface,
    sockets: SocketSet<'static>,
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
        })
    }

    /// 注入一帧接收数据。
    pub fn push_frame(&mut self, frame: &[u8]) -> Result<(), NetError> {
        if self.device.push_rx(frame) {
            Ok(())
        } else {
            Err(NetError::Queue)
        }
    }

    /// 取出一帧待发送数据。
    pub fn take_frame(&mut self) -> Option<Vec<u8>> {
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
