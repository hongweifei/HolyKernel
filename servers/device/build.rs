//! 链接配置：服务镜像固定在低地址 0x00600000，与其它服务区分。

fn main() {
    println!("cargo:rerun-if-changed=linker.ld");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "none" {
        return;
    }

    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR 存在");
    println!("cargo:rustc-link-arg=-T{manifest}/linker.ld");
    println!("cargo:rustc-link-arg=--build-id=none");
    println!("cargo:rustc-link-arg=-no-pie");
}
