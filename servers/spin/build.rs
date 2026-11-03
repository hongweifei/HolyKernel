//! 链接配置：spin 服务固定链接到 0x00700000。

fn main() {
    hk_server_build::configure(0x0070_0000);
}
