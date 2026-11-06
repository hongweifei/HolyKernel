//! 服务二进制与镜像模块清单（单一事实来源）。
//!
//! 清单与自动镜像基址分配都在 `hk-server-build` 中维护。

pub use hk_server_build::SERVERS;

/// 随镜像打包的 HolyShell 脚本。
pub const SCRIPT_MODULES: &[&str] = &["boot.hsh", "services.hsh"];
