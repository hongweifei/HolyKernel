//! 链接配置：路径兼容服务固定链接到 0x00C00000。

fn main() {
    hk_server_build::configure(0x00C0_0000);
}
