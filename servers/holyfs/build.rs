//! 链接配置：HolyFS 服务固定链接到 0x01200000。

fn main() {
    hk_server_build::configure(0x0120_0000);
}
