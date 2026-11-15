//! 从 virtio-gpu 接管扫描输出。
//!
//! Display 原本消费内核交出的固件帧缓冲；在没有线性帧缓冲（例如只挂
//! `virtio-gpu-pci`）或显式要求 `gpu.scanout` 时，改为向 virtio 驱动服务索取
//! 扫描输出区域：驱动把资源 backing 的能力句柄复制一份交过来，本服务映射后
//! 直接写像素，每帧有 damage 时再让驱动 `flush` 到宿主。
//!
//! 接管发生在服务入口、合成器启动之前，此时客户端可能已经把窗口请求发到本
//! 服务的端点上。握手期间收到的不相关消息会被原样排进 [`Pending`]，合成器
//! 启动后按顺序重放，不能因为等 GPU 回复而丢掉客户端的首个请求。

use alloc::vec::Vec;

use hk_proto::scanout::{
    Client, SCANOUT_FLUSH, SCANOUT_PING, SCANOUT_PONG, SCANOUT_REGION, SCANOUT_REGION_INFO,
    SCANOUT_SERVICE,
};
use hk_server_api::{map_attach, recv_data_handle_try, send, yield_now};

/// 接管期间缓存的消息载荷上限（与合成器的接收缓冲一致）。
pub(crate) const PENDING_PAYLOAD: usize = 256;

/// 接管期间暂存的客户端消息。
pub(crate) struct Pending {
    pub(crate) label: u64,
    pub(crate) word: u64,
    pub(crate) sender: u64,
    pub(crate) length: usize,
    pub(crate) handle: u64,
    pub(crate) payload: [u8; PENDING_PAYLOAD],
}

/// 接管来的扫描输出。
pub(crate) struct Scanout {
    /// 映射后的像素起点。
    pub(crate) fb: *mut u32,
    /// 像素宽度。
    pub(crate) width: usize,
    /// 像素高度。
    pub(crate) height: usize,
    /// 每行像素数。
    pub(crate) stride: usize,
    client: Client,
    /// 能力句柄与 Region 在服务生命周期内保持打开，映射因此有效。
    _capability: u64,
    _region: u64,
}

impl Scanout {
    /// 等待驱动服务并接管扫描输出；期间到达的客户端消息记入 `pending`。
    pub(crate) fn adopt(endpoint: u64, pending: &mut Vec<Pending>) -> Option<Self> {
        let service = wait_service(SCANOUT_SERVICE)?;
        // 先确认服务在跑，再索取区域；服务尚未进入主循环时请求会排队。
        let mut buffer = [0_u8; PENDING_PAYLOAD];
        let mut attempts = 0;
        loop {
            let _ = send(service, SCANOUT_PING, 0);
            let mut answered = false;
            for _ in 0..256 {
                let (label, word, sender, length, handle) =
                    recv_data_handle_try(endpoint, &mut buffer, 0);
                if label == 0 {
                    let _ = yield_now();
                    continue;
                }
                if sender == service && label == SCANOUT_PONG {
                    answered = true;
                    break;
                }
                queue(pending, label, word, sender, length, handle, &buffer);
            }
            if answered {
                break;
            }
            attempts += 1;
            if attempts >= 8 {
                return None;
            }
        }

        let _ = send(service, SCANOUT_REGION, 0);
        let mut geometry = None;
        let mut capability = 0;
        for _ in 0..20_000 {
            let (label, word, sender, length, handle) =
                recv_data_handle_try(endpoint, &mut buffer, 0);
            if label == 0 {
                let _ = yield_now();
                continue;
            }
            if sender == service {
                match label {
                    SCANOUT_REGION_INFO if length >= 12 => {
                        geometry = Some((
                            u32::from_le_bytes(buffer[0..4].try_into().ok()?),
                            u32::from_le_bytes(buffer[4..8].try_into().ok()?),
                            u32::from_le_bytes(buffer[8..12].try_into().ok()?),
                        ));
                    }
                    SCANOUT_REGION if handle != 0 => capability = handle,
                    _ => {}
                }
            } else {
                queue(pending, label, word, sender, length, handle, &buffer);
            }
            if capability != 0
                && let Some((width, height, stride)) = geometry
            {
                let (region, virt) = map_attach(capability)?;
                return Some(Self {
                    fb: virt as *mut u32,
                    width: width as usize,
                    height: height as usize,
                    stride: stride as usize,
                    client: Client::at(endpoint, service),
                    _capability: capability,
                    _region: region,
                });
            }
        }
        None
    }

    /// 请求驱动把本帧提交给宿主。
    ///
    /// 只发请求不等回复：Display 的端点同时承载客户端消息，阻塞等待会吞掉
    /// 客户端的表面提交；驱动的 `SCANOUT_FLUSH_ACK` 由主循环按未知标签忽略。
    pub(crate) fn flush(&self) {
        let _ = hk_server_api::send(self.client.service(), SCANOUT_FLUSH, 0);
    }
}

/// 缓存一条与接管无关的客户端消息，交给合成器重放。
fn queue(
    pending: &mut Vec<Pending>,
    label: u64,
    word: u64,
    sender: u64,
    length: usize,
    handle: u64,
    buffer: &[u8],
) {
    let length = length.min(PENDING_PAYLOAD);
    let mut payload = [0_u8; PENDING_PAYLOAD];
    payload[..length].copy_from_slice(&buffer[..length]);
    pending.push(Pending {
        label,
        word,
        sender,
        length,
        handle,
        payload,
    });
}

/// 等待服务名对应的端点；超时返回 `None`。
fn wait_service(name: &str) -> Option<u64> {
    for _ in 0..hk_server_api::runtime::SERVICE_WAIT_ATTEMPTS {
        let endpoint = hk_server_api::service_endpoint(name);
        if endpoint != 0 {
            return Some(endpoint);
        }
        let _ = yield_now();
    }
    None
}
