//! 链接配置：用户态 Shell 服务固定链接到 0x01C00000。

fn main() {
    hk_server_build::configure(0x01C0_0000);
}
