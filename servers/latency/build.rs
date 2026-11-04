//! 链接配置：latency 服务固定链接到 0x00B00000。

fn main() {
    hk_server_build::configure(0x00B0_0000);
}
