//! 链接配置：netstat 客户端固定链接到 0x01100000。

fn main() {
    hk_server_build::configure(0x0110_0000);
}
