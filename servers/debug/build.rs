//! 链接配置：Debug 服务固定链接到 0x01B00000。

fn main() {
    hk_server_build::configure(0x01B0_0000);
}
