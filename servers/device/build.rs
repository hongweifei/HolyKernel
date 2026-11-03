//! 链接配置：device 服务固定链接到 0x00600000。

fn main() {
    hk_server_build::configure(0x0060_0000);
}
