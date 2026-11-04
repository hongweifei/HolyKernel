//! 通用设备授权协议：驱动按设备类申请授权。

/// 驱动请求下发设备授权。
pub const DEVICE_REQUEST: u64 = 0x604;
/// DeviceGrant 移交标签。
pub const DEVICE_GRANT: u64 = 0x602;
/// 没有匹配设备。
pub const DEVICE_NONE: u64 = 0x603;

/// 任意 virtio 设备。
pub const DEVICE_CLASS_ANY: u64 = 0;
/// 块设备。
pub const DEVICE_CLASS_BLOCK: u64 = 1;
/// 网络设备。
pub const DEVICE_CLASS_NETWORK: u64 = 2;
