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

/// 以太网帧最大长度。
pub const NET_FRAME_MAX: usize = 1514;
/// 接口信息编码长度：MAC + MTU + link。
pub const NET_INFO_BYTES: usize = 6 + 2 + 1;
