//! 链接配置：virtio-net 驱动服务固定链接到 0x00F00000。

fn main() {
    hk_server_build::configure(0x00F0_0000);
}
