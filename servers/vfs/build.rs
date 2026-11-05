//! 链接配置：原生 VFS 服务固定链接到 0x01400000。

fn main() {
    hk_server_build::configure(0x0140_0000);
}
