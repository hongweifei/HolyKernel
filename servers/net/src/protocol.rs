//! 网络服务线协议：状态编码、Socket 请求与共享环回传。

use alloc::vec::Vec;
use core::net::Ipv4Addr;

use hk_net::{DnsQueryType, IpAddress, SocketId, Stack};
use hk_server_api::net::{
    NET_NOTIFY_TX, NET_POLL, NET_RX_REFILL, NET_TX, NET_TX_FRAME, NetStatus, net_tx_word,
};
use hk_server_api::socket::{
    SOCK_CLOSE, SOCK_CONNECT, SOCK_ERROR, SOCK_KIND_TCP, SOCK_KIND_UDP, SOCK_OPEN,
    SOCK_PAYLOAD_MAX, SOCK_RECV, SOCK_RESOLVE, SOCK_RESPONSE, SOCK_SEND, SOCK_UDP_RECV,
    SOCK_UDP_SEND,
};
use hk_server_api::{notify, send, send_data};

use crate::{DNS_WAIT_MS, NET_FRAME_MAX, Stats, now_ms, poll_once};

/// 把接口状态与收发计数编码为线协议字节串。
#[cfg(target_os = "none")]
pub(crate) fn status_blob(stack: &Stack, stats: &Stats, buffer: &mut [u8]) -> Option<usize> {
    let status = stack.status();
    NetStatus {
        mac: status.mac,
        ipv4: status.ipv4.map_or([0; 4], |address| address.octets()),
        has_ipv4: status.ipv4.is_some(),
        prefix: status.prefix,
        gateway: status.gateway.map_or([0; 4], |address| address.octets()),
        has_gateway: status.gateway.is_some(),
        sockets: status.sockets as u16,
        rx_frames: stats.rx_frames,
        tx_frames: stats.tx_frames,
        rx_bytes: stats.rx_bytes,
        tx_bytes: stats.tx_bytes,
    }
    .encode(buffer)
}

/// 处理一次原生 Socket 请求并回复。
#[cfg(target_os = "none")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_socket(
    label: u64,
    data: &[u8],
    sender: u64,
    driver: u64,
    endpoint: u64,
    stack: &mut Stack,
    stats: &mut Stats,
    dns_servers: &[Ipv4Addr],
    frame: &mut [u8; NET_FRAME_MAX],
    next_udp: &mut u16,
) {
    let mut reply = [0_u8; 8 + SOCK_PAYLOAD_MAX];
    let mut payload = 0_usize;
    let result = match label {
        SOCK_OPEN => {
            let id = match data.first().copied() {
                Some(SOCK_KIND_TCP) => stack.socket_open_tcp(2048),
                Some(SOCK_KIND_UDP) => {
                    let port = *next_udp;
                    *next_udp = next_udp.wrapping_add(1);
                    stack.socket_open_udp(port)
                }
                _ => None,
            };
            id.map_or(SOCK_ERROR, |id| id.0 as u64 + 1)
        }
        SOCK_CONNECT => match (socket_id(data, 0), read_ipv4(data, 8), read_u16(data, 12)) {
            (Some(id), Some(address), Some(port)) => {
                if stack.socket_connect(id, Ipv4Addr::from(address), port, 40000 + id.0 as u16) {
                    0
                } else {
                    SOCK_ERROR
                }
            }
            _ => SOCK_ERROR,
        },
        SOCK_SEND => match socket_id(data, 0) {
            Some(id) => stack.socket_send(id, &data[8..]).unwrap_or(0) as u64,
            None => SOCK_ERROR,
        },
        SOCK_RECV => {
            let max = read_u16(data, 8)
                .unwrap_or(SOCK_PAYLOAD_MAX as u16)
                .min(SOCK_PAYLOAD_MAX as u16) as usize;
            let mut buffer = [0_u8; SOCK_PAYLOAD_MAX];
            match socket_id(data, 0) {
                Some(id) => match stack.socket_recv(id, &mut buffer[..max]) {
                    Ok(Some(length)) => {
                        reply[8..8 + length].copy_from_slice(&buffer[..length]);
                        payload = length;
                        length as u64
                    }
                    Ok(None) => 0,
                    Err(_) => SOCK_ERROR,
                },
                None => SOCK_ERROR,
            }
        }
        SOCK_UDP_RECV => {
            let max = read_u16(data, 8)
                .unwrap_or(SOCK_PAYLOAD_MAX as u16)
                .min(SOCK_PAYLOAD_MAX as u16) as usize;
            let mut buffer = [0_u8; SOCK_PAYLOAD_MAX];
            match socket_id(data, 0) {
                Some(id) => match stack.socket_udp_recv(id, &mut buffer[..max]) {
                    Ok(Some(length)) => {
                        reply[8..8 + length].copy_from_slice(&buffer[..length]);
                        payload = length;
                        length as u64
                    }
                    Ok(None) => 0,
                    Err(_) => SOCK_ERROR,
                },
                None => SOCK_ERROR,
            }
        }
        SOCK_CLOSE => match socket_id(data, 0) {
            Some(id) => {
                stack.socket_close(id);
                0
            }
            None => SOCK_ERROR,
        },
        SOCK_UDP_SEND => match (socket_id(data, 0), read_ipv4(data, 8), read_u16(data, 12)) {
            (Some(id), Some(address), Some(port)) => {
                if stack.socket_udp_send(id, Ipv4Addr::from(address), port, &data[14..]) {
                    (data.len() - 14) as u64
                } else {
                    0
                }
            }
            _ => SOCK_ERROR,
        },
        SOCK_RESOLVE => {
            let name = core::str::from_utf8(data).unwrap_or("");
            let fallback = [Ipv4Addr::new(10, 0, 2, 3)];
            let servers = if dns_servers.is_empty() {
                &fallback[..]
            } else {
                dns_servers
            };
            match lookup(stack, servers, name, driver, endpoint, stats, frame) {
                Some(addresses) => {
                    let count = addresses.len().min(4);
                    for (index, address) in addresses.iter().take(count).enumerate() {
                        reply[8 + index * 4..8 + index * 4 + 4].copy_from_slice(&address.octets());
                    }
                    payload = count * 4;
                    count as u64
                }
                None => SOCK_ERROR,
            }
        }
        _ => SOCK_ERROR,
    };
    reply[..8].copy_from_slice(&result.to_le_bytes());
    let _ = send_data(sender, SOCK_RESPONSE, &reply[..8 + payload]);
    stack.poll(now_ms());
    flush_tx(driver, stack, stats);
    // 主动轮询驱动，确保握手/重传期间能及时取走接收帧。
    let _ = send(driver, NET_POLL, 0);
    let _ = notify(driver, NET_NOTIFY_TX);
}

#[cfg(target_os = "none")]
fn socket_id(data: &[u8], offset: usize) -> Option<SocketId> {
    let handle = read_u64(data, offset)?;
    (handle != 0).then(|| SocketId((handle - 1) as usize))
}

#[cfg(target_os = "none")]
fn read_u64(data: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        data.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

#[cfg(target_os = "none")]
fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        data.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

#[cfg(target_os = "none")]
fn read_ipv4(data: &[u8], offset: usize) -> Option<[u8; 4]> {
    data.get(offset..offset + 4)?.try_into().ok()
}

/// 用协议栈解析名称，返回最多 4 个 IPv4 地址。
#[cfg(target_os = "none")]
fn lookup(
    stack: &mut Stack,
    servers: &[Ipv4Addr],
    name: &str,
    driver: u64,
    endpoint: u64,
    stats: &mut Stats,
    frame: &mut [u8; NET_FRAME_MAX],
) -> Option<Vec<Ipv4Addr>> {
    let dns = stack.open_dns(servers).ok()?;
    let query = match stack.start_dns_query(dns, name, DnsQueryType::A) {
        Ok(query) => query,
        Err(_) => {
            stack.close_socket(dns);
            return None;
        }
    };
    let deadline = now_ms().saturating_add(DNS_WAIT_MS);
    let mut result = None;
    while now_ms() < deadline {
        poll_once(driver, endpoint, stack, stats, frame);
        match stack.poll_dns(dns, query) {
            Ok(Some(addresses)) => {
                result = Some(
                    addresses
                        .iter()
                        .filter_map(|address| match address {
                            IpAddress::Ipv4(address) => Some(*address),
                            _ => None,
                        })
                        .take(4)
                        .collect(),
                );
                break;
            }
            Ok(None) => {}
            Err(_) => break,
        }
    }
    stack.close_socket(dns);
    result
}

/// 回传共享环的归还/发送事件，并补发普通发送队列；随后唤醒驱动。
#[cfg(target_os = "none")]
pub(crate) fn flush_tx(driver: u64, stack: &mut Stack, stats: &mut Stats) {
    let mut rx_done = Vec::new();
    let mut tx_ready = Vec::new();
    stack.take_shared_events(&mut rx_done, &mut tx_ready);
    let mut woke = false;
    for index in rx_done {
        let _ = send(driver, NET_RX_REFILL, index as u64);
        woke = true;
    }
    for (index, length) in tx_ready {
        stats.tx_frames = stats.tx_frames.saturating_add(1);
        stats.tx_bytes = stats.tx_bytes.saturating_add(length as u64);
        let _ = send(driver, NET_TX_FRAME, net_tx_word(index, length));
        woke = true;
    }
    while let Some(packet) = stack.take_frame() {
        stats.tx_frames = stats.tx_frames.saturating_add(1);
        stats.tx_bytes = stats.tx_bytes.saturating_add(packet.len() as u64);
        let _ = send_data(driver, NET_TX, &packet);
        woke = true;
    }
    if woke {
        let _ = notify(driver, NET_NOTIFY_TX);
    }
}
