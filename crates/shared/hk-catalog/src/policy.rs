//! OOM、GPU 容量与 netstat 策略名单。

/// OOM 选择时豁免的服务：内存、调试、检查点、任务管理器与观察端。
pub fn oom_exempt(name: &str) -> bool {
    name == "root"
        || name.starts_with("hk-server-memory")
        || name == "hk-client-oom-observer"
        || name.starts_with("hk-server-debug")
        || name.starts_with("hk-server-checkpoint")
        || name.starts_with("hk-server-taskman")
}

/// 需要独立 Gpu Capacity 的服务。
pub const GPU_CAPACITY_SERVICES: &[&str] = &["hk-server-gpu-provider", "hk-client-gpu-test"];

/// 每个 Gpu 服务取得的 Capacity 字节数。
pub const GPU_CAPACITY_BYTES: usize = 1024 * 1024;

/// 判断服务能否在自检标记下修改 OOM 策略。
pub fn oom_policy_authority(name: &str, cmdline: &str) -> bool {
    name == "hk-client-oom-test" && cmdline.contains("oom-test")
}

/// 内核态 shell 的 `net` 命令查询的状态客户端。
pub const NETSTAT_CLIENT: &str = "hk-client-netstat";
