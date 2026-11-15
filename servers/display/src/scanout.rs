//! 从 virtio-gpu 接管扫描输出。
//!
//! Display 原本消费内核交出的固件帧缓冲；在没有线性帧缓冲（例如只挂
//! `virtio-gpu-pci`）或显式要求 `gpu.scanout` 时，改为向 virtio 驱动服务索取
//! 扫描输出区域：驱动把资源 backing 的能力句柄复制一份交过来，本服务映射后
//! 直接写像素，每帧结束再让驱动 `flush` 到宿主。

use hk_proto::scanout::Client;
use hk_server_api::{map_attach, yield_now};

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
    /// 等待驱动服务并接管扫描输出；不可用时返回 `None`。
    pub(crate) fn adopt(endpoint: u64) -> Option<Self> {
        let mut attempts = 0;
        let client = loop {
            if let Some(client) = Client::discover(endpoint)
                && client.ping()
            {
                break client;
            }
            attempts += 1;
            if attempts >= 8 {
                return None;
            }
            let _ = yield_now();
        };
        let Some(region) = client.region() else {
            hk_server_api::write(b"hk-server-display: scanout region failed\n");
            return None;
        };
        let Some((mapped, virt)) = map_attach(region.capability) else {
            hk_server_api::write(b"hk-server-display: scanout map failed\n");
            return None;
        };
        Some(Self {
            fb: virt as *mut u32,
            width: region.width as usize,
            height: region.height as usize,
            stride: region.stride as usize,
            client,
            _capability: region.capability,
            _region: mapped,
        })
    }

    /// 把本帧提交给宿主。
    pub(crate) fn flush(&self) {
        let _ = self.client.flush();
    }
}
