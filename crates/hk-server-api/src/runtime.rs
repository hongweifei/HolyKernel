//! 服务运行时辅助：服务发现。

use crate::{service_endpoint, yield_now};

/// 等待服务出现的默认重试次数。
pub const SERVICE_WAIT_ATTEMPTS: usize = 4096;

/// 等待按名称注册的服务；超时返回 `None`。
pub fn wait_service(name: &str) -> Option<u64> {
    for _ in 0..SERVICE_WAIT_ATTEMPTS {
        let endpoint = service_endpoint(name);
        if endpoint != 0 {
            return Some(endpoint);
        }
        let _ = yield_now();
    }
    None
}
