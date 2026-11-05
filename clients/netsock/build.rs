//! 链接配置：Socket 自检服务固定链接到 0x01200000。

fn main() {
    hk_server_build::configure(0x0120_0000);
}
