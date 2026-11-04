//! 链接配置：fault 服务固定链接到 0x00900000。

fn main() {
    hk_server_build::configure(0x0090_0000);
}
