//! 链接配置：Input 服务固定链接到 0x01800000。

fn main() {
    hk_server_build::configure(0x0180_0000);
}
