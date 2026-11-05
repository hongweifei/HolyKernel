//! 链接配置：Debug 客户端固定链接到 0x01D00000。

fn main() {
    hk_server_build::configure(0x01D0_0000);
}
