//! 链接配置：fork 服务固定链接到 0x00A00000。

fn main() {
    hk_server_build::configure(0x00A0_0000);
}
