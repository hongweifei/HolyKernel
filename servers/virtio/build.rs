//! 链接配置：virtio 驱动服务固定链接到 0x00E00000。

fn main() {
    hk_server_build::configure(0x00E0_0000);
}
