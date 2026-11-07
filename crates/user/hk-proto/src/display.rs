//! 显示服务协议：窗口创建、表面提交与帧完成。
//!
//! 客户端分配共享 Frame 作为窗口表面，绘制后把句柄随创建请求交给显示服务；
//! 显示服务把表面映射进自己的地址空间并合成到帧缓冲，不需要复制整块像素。

pub use hk_syscall::DISPLAY_SERVICE;

use hk_server_api::{
    CapabilityKind, MemoryClient, close_handle, exit, recv, recv_data, region_unmap, send,
    send_handle,
};

/// 客户端窗口：表面缓冲、窗口编号与提交。
pub struct Window {
    endpoint: u64,
    display: u64,
    surface: u64,
    window: u64,
    region: u64,
    virt: u64,
    width: usize,
    height: usize,
}

impl Window {
    /// 创建共享表面并挂到 Display，返回已映射的窗口。
    pub fn attach(endpoint: u64, display: u64, width: usize, height: usize) -> Option<Self> {
        let bytes = width.checked_mul(height)?.checked_mul(4)?;
        let (handle, region, virt) =
            MemoryClient::new(endpoint).map(CapabilityKind::Surface, bytes)?;
        if send_handle(
            display,
            SURFACE_CREATE,
            pack_size(width as u32, height as u32),
            handle,
        ) == u64::MAX
        {
            let _ = close_handle(handle);
            let _ = region_unmap(region);
            return None;
        }
        let (label, surface, _sender) = recv(endpoint);
        if label != SURFACE_CREATED || surface == 0 {
            let _ = region_unmap(region);
            return None;
        }
        if send(display, WINDOW_CREATE, surface) == u64::MAX {
            let _ = region_unmap(region);
            return None;
        }
        let (label, window, _sender) = recv(endpoint);
        if label != WINDOW_CREATED || window == 0 {
            let _ = region_unmap(region);
            return None;
        }
        Some(Self {
            endpoint,
            display,
            surface,
            window,
            region,
            virt,
            width,
            height,
        })
    }

    /// 替换窗口表面；成功后可继续使用同一 surface/window 编号。
    pub fn resize(&mut self, width: usize, height: usize) -> bool {
        if width == 0 || height == 0 || (width == self.width && height == self.height) {
            return true;
        }
        let Some(bytes) = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4))
        else {
            return false;
        };
        let Some((handle, region, virt)) =
            MemoryClient::new(self.endpoint).map(CapabilityKind::Surface, bytes)
        else {
            return false;
        };
        if send_handle(
            self.display,
            WINDOW_RESIZE,
            pack_window_size(self.window, width as u32, height as u32),
            handle,
        ) == u64::MAX
        {
            let _ = close_handle(handle);
            let _ = region_unmap(region);
            return false;
        }
        let old_region = self.region;
        self.region = region;
        self.virt = virt;
        self.width = width;
        self.height = height;
        let _ = region_unmap(old_region);
        true
    }

    /// 设置窗口标题。
    pub fn set_title(&self, title: &str) {
        set_title(self.display, self.window, title);
    }

    /// 提交并等待 Display 完成合成。
    pub fn commit(&self) {
        self.commit_with(|_, _, _| {});
    }

    /// 提交并等待合成；等待期间把其他消息交给调用方处理。
    pub fn commit_with<F>(&self, mut on_message: F)
    where
        F: FnMut(u64, u64, &[u8]),
    {
        let _ = send(self.display, SURFACE_COMMIT, self.surface);
        let mut buffer = [0_u8; 2048];
        loop {
            let (label, word, _sender, length) = recv_data(self.endpoint, &mut buffer);
            match label {
                SURFACE_FRAME_DONE if word == self.surface => return,
                DISPLAY_CLOSE => exit(0),
                _ => on_message(label, word, &buffer[..length.min(buffer.len())]),
            }
        }
    }

    /// 提交但不等待合成完成，供已经有独立背压路径的终端使用。
    pub fn commit_no_wait(&self) {
        let _ = send(self.display, SURFACE_COMMIT, self.surface);
    }

    /// 解除本地表面映射。
    pub fn detach(&self) {
        let _ = region_unmap(self.region);
    }

    /// 返回表面编号。
    pub const fn surface(&self) -> u64 {
        self.surface
    }

    /// 返回窗口编号。
    pub const fn window(&self) -> u64 {
        self.window
    }

    /// 返回显示服务端点编号。
    pub const fn display(&self) -> u64 {
        self.display
    }

    /// 返回表面像素指针。
    pub const fn pixels(&self) -> *mut u32 {
        self.virt as *mut u32
    }

    /// 返回内容宽度。
    pub const fn width(&self) -> usize {
        self.width
    }

    /// 返回内容高度。
    pub const fn height(&self) -> usize {
        self.height
    }
}

/// 请求显示服务退出（用于自检收尾）。
pub const DISPLAY_SHUTDOWN: u64 = 0xA04;
/// 键盘事件；`word` 为打包的窗口编号与字节，发给焦点窗口的客户端。
pub const DISPLAY_KEY: u64 = 0xA05;
/// 设置窗口标题；载荷为 8 字节窗口编号 + UTF-8 标题。
pub const DISPLAY_SET_TITLE: u64 = 0xA06;
/// 窗口被鼠标点击；`word` 为窗口编号。
pub const DISPLAY_CLICK: u64 = 0xA07;
/// 窗口被关闭；`word` 为窗口编号。
pub const DISPLAY_CLOSE: u64 = 0xA08;
/// 窗口正文点击位置；`word` 打包窗口编号与窗口内坐标。
pub const DISPLAY_CLICK_AT: u64 = 0xA09;
/// 请求 Display 以失败码退出，供热重启自检使用。
pub const DISPLAY_RESTART: u64 = 0xA0A;
/// 鼠标滚轮；`word` 打包窗口编号与有符号增量，发给焦点窗口的客户端。
pub const DISPLAY_WHEEL: u64 = 0xA0B;
/// 窗口内容尺寸变化；`word` 打包窗口编号与新的内容宽高，发给窗口客户端。
pub const DISPLAY_RESIZE: u64 = 0xA0C;
/// 指针在窗口正文内移动；`word` 打包窗口编号与窗口内坐标。
pub const DISPLAY_POINTER: u64 = 0xA1E;
/// 指针离开窗口正文；`word` 为窗口编号。
pub const DISPLAY_POINTER_LEAVE: u64 = 0xA1F;
/// 设置表面光标形状；`word` 由 [`crate::pointer::pack_surface_cursor`] 打包。
pub const SURFACE_SET_CURSOR: u64 = 0xA20;

/// 创建表面；`word` 打包宽高，随消息转移表面 Frame 句柄。
pub const SURFACE_CREATE: u64 = 0xA10;
/// 表面已创建；`word` 为表面编号。
pub const SURFACE_CREATED: u64 = 0xA11;
/// 标记表面 damage；`word` 为表面编号，载荷为 [`SURFACE_RECT_BYTES`] 字节矩形。
pub const SURFACE_DAMAGE: u64 = 0xA12;
/// 设置表面输入区域；`word` 为表面编号，载荷为矩形，空矩形表示整个表面。
pub const SURFACE_SET_INPUT_REGION: u64 = 0xA13;
/// 提交表面；`word` 为表面编号。
pub const SURFACE_COMMIT: u64 = 0xA14;
/// 表面一帧已合成；`word` 为表面编号。
pub const SURFACE_FRAME_DONE: u64 = 0xA15;
/// 销毁表面；`word` 为表面编号。
pub const SURFACE_DESTROY: u64 = 0xA16;
/// 把表面挂到窗口；`word` 为表面编号，回复 [`WINDOW_CREATED`]。
pub const WINDOW_CREATE: u64 = 0xA17;
/// 窗口已创建；`word` 为窗口编号。
pub const WINDOW_CREATED: u64 = 0xA18;
/// 客户端替换窗口表面；`word` 打包窗口编号与新表面宽高，随消息转移 Frame 句柄。
pub const WINDOW_RESIZE: u64 = 0xA1D;
/// 设置焦点窗口；`word` 为窗口编号。
pub const SEAT_FOCUS: u64 = 0xA19;
/// 写入剪贴板；载荷为 UTF-8 文本。
pub const SEAT_SET_CLIPBOARD: u64 = 0xA1A;
/// 请求剪贴板；回复 [`SEAT_CLIPBOARD`]。
pub const SEAT_GET_CLIPBOARD: u64 = 0xA1B;
/// 剪贴板内容；载荷为 UTF-8 文本。
pub const SEAT_CLIPBOARD: u64 = 0xA1C;

/// 表面矩形载荷长度。
pub const SURFACE_RECT_BYTES: usize = 8;
/// 表面编号 + 矩形载荷长度。
pub const SURFACE_PAYLOAD_BYTES: usize = 16;

/// 窗口标题最大字节数。
pub const TITLE_MAX: usize = 32;

/// 设置窗口标题（非阻塞，显示服务下一轮处理）。
pub fn set_title(display: u64, window: u64, title: &str) {
    let mut buffer = [0_u8; 8 + TITLE_MAX];
    buffer[..8].copy_from_slice(&window.to_le_bytes());
    let mut length = title.len().min(TITLE_MAX);
    while !title.is_char_boundary(length) {
        length -= 1;
    }
    buffer[8..8 + length].copy_from_slice(&title.as_bytes()[..length]);
    let _ = hk_server_api::send_data(display, DISPLAY_SET_TITLE, &buffer[..8 + length]);
}

/// 把表面坐标矩形编码为 4 个小端 u16。
pub const fn pack_rect(x: u16, y: u16, width: u16, height: u16) -> [u8; SURFACE_RECT_BYTES] {
    [
        x as u8,
        (x >> 8) as u8,
        y as u8,
        (y >> 8) as u8,
        width as u8,
        (width >> 8) as u8,
        height as u8,
        (height >> 8) as u8,
    ]
}

/// 从载荷解出表面坐标矩形；长度不足时返回 `None`。
pub fn unpack_rect(bytes: &[u8]) -> Option<(u16, u16, u16, u16)> {
    if bytes.len() < SURFACE_RECT_BYTES {
        return None;
    }
    let word = |offset: usize| u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
    Some((word(0), word(2), word(4), word(6)))
}

/// 打包表面编号与矩形，用于 damage 与输入区域请求。
pub fn pack_surface_rect(
    surface: u64,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
) -> [u8; SURFACE_PAYLOAD_BYTES] {
    let mut bytes = [0_u8; SURFACE_PAYLOAD_BYTES];
    bytes[..8].copy_from_slice(&surface.to_le_bytes());
    bytes[8..].copy_from_slice(&pack_rect(x, y, width, height));
    bytes
}

/// 从载荷解出表面编号与矩形。
pub fn unpack_surface_rect(bytes: &[u8]) -> Option<(u64, u16, u16, u16, u16)> {
    if bytes.len() < SURFACE_PAYLOAD_BYTES {
        return None;
    }
    let surface = u64::from_le_bytes(bytes[..8].try_into().ok()?);
    let (x, y, width, height) = unpack_rect(&bytes[8..])?;
    Some((surface, x, y, width, height))
}

/// 把宽高打包进一个消息字。
pub const fn pack_size(width: u32, height: u32) -> u64 {
    ((width as u64) << 32) | height as u64
}

/// 从消息字解包宽高。
pub const fn unpack_size(word: u64) -> (u32, u32) {
    ((word >> 32) as u32, word as u32)
}

/// 把窗口编号与内容宽高打包进一个消息字；宽高限制在 16 位。
pub const fn pack_window_size(window: u64, width: u32, height: u32) -> u64 {
    (window << 32) | ((width as u64 & 0xFFFF) << 16) | (height as u64 & 0xFFFF)
}

/// 从消息字解包窗口编号与内容宽高。
pub const fn unpack_window_size(word: u64) -> (u64, u32, u32) {
    (
        word >> 32,
        ((word >> 16) & 0xFFFF) as u32,
        (word & 0xFFFF) as u32,
    )
}

/// 把窗口编号与按键字节打包进一个消息字。
pub const fn pack_key(window: u64, byte: u8) -> u64 {
    (window << 32) | byte as u64
}

/// 从消息字解包窗口编号与按键字节。
pub const fn unpack_key(word: u64) -> (u64, u8) {
    (word >> 32, word as u8)
}

/// 把窗口编号与滚轮增量打包进一个消息字。
pub const fn pack_wheel(window: u64, delta: i32) -> u64 {
    (window << 32) | delta as u32 as u64
}

/// 从消息字解包窗口编号与滚轮增量。
pub const fn unpack_wheel(word: u64) -> (u64, i32) {
    (word >> 32, word as u32 as i32)
}

/// 把窗口编号与窗口内坐标打包进一个消息字。
pub const fn pack_click_at(window: u64, x: u32, y: u32) -> u64 {
    (window << 32) | ((x as u64 & 0xFFFF) << 16) | (y as u64 & 0xFFFF)
}

/// 从消息字解包窗口编号与窗口内坐标。
pub const fn unpack_click_at(word: u64) -> (u64, u32, u32) {
    (
        word >> 32,
        ((word >> 16) & 0xFFFF) as u32,
        (word & 0xFFFF) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::{pack_window_size, unpack_window_size};

    #[test]
    fn packs_window_size_and_position() {
        let word = pack_window_size(3, 464, 348);
        assert_eq!(unpack_window_size(word), (3, 464, 348));
    }
}
