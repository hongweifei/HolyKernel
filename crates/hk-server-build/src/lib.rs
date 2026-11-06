//! 服务镜像链接配置。
//!
//! 每个服务运行在自己的用户页表中，镜像基址本身不需要全局唯一；这里仍按
//! [`SERVERS`] 顺序为每个服务分配互不重叠的槽位，使地址能稳定对应到服务，
//! 供故障回溯与离线符号化使用。构建脚本调用 [`configure`] 即可，不再手写
//! 链接地址。

use std::path::PathBuf;

/// 随镜像构建的服务；顺序同时决定自动分配的镜像基址。
pub const SERVERS: &[&str] = &[
    "hk-server-demo",
    "hk-server-echo",
    "hk-server-device",
    "hk-server-spin",
    "hk-server-virtio",
    "hk-server-holyfs",
    "hk-server-vfs",
    "hk-server-path",
    "hk-server-fault",
    "hk-server-fork",
    "hk-server-latency",
    "hk-server-block",
    "hk-server-virtio-net",
    "hk-server-net",
    "hk-server-netstat",
    "hk-server-netsock",
    "hk-server-console",
    "hk-server-console-test",
    "hk-server-shell",
    "hk-server-display",
    "hk-server-display-test",
    "hk-server-terminal-test",
    "hk-server-input",
    "hk-server-pointer-test",
    "hk-server-holydoc",
    "hk-server-workbench",
    "hk-server-debug",
    "hk-server-dbg",
    "hk-server-display-restart-test",
    "hk-server-checkpoint",
    "hk-server-checkpoint-test",
];

/// 服务镜像区的起始地址。
///
/// 512 MiB：低于固定映射（网络 768 MiB、surface 1.25 GiB 等）与 x86_64
/// 小代码模型的 2 GiB 上限。
pub const IMAGE_BASE: u64 = 0x2000_0000;

/// 每个服务镜像槽位的长度。
pub const IMAGE_STRIDE: u64 = 0x0040_0000;

/// 返回服务的镜像链接基址；服务未登记时返回 `None`。
pub fn image_base(name: &str) -> Option<u64> {
    SERVERS
        .iter()
        .position(|service| *service == name)
        .map(|index| IMAGE_BASE + index as u64 * IMAGE_STRIDE)
}

/// 为当前包生成链接脚本。
///
/// 基址由 [`SERVERS`] 中的顺序自动分配，并通过 `HK_IMAGE_BASE` 暴露给
/// 服务本身（例如调试服务用自身基址做符号化自检）。
pub fn configure() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "none" {
        return;
    }

    let name = std::env::var("CARGO_PKG_NAME").expect("构建脚本需要 CARGO_PKG_NAME");
    let Some(base) = image_base(&name) else {
        panic!("服务 {name} 未登记在 hk-server-build::SERVERS 中");
    };
    println!("cargo:rustc-env=HK_IMAGE_BASE=0x{base:x}");
    configure_at(base);
}

/// 用显式基址生成链接脚本；仅供测试或特殊镜像使用。
pub fn configure_at(base: u64) {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "none" {
        return;
    }

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR 存在"));
    let linker = out_dir.join("linker.ld");
    std::fs::write(&linker, linker_script(base)).expect("写入 linker.ld");

    println!("cargo:rustc-link-arg=-T{}", linker.display());
    println!("cargo:rustc-link-arg=--build-id=none");
    println!("cargo:rustc-link-arg=-no-pie");
}

fn linker_script(base: u64) -> String {
    format!(
        "OUTPUT_FORMAT(elf64-x86-64)
ENTRY(_start)

PHDRS
{{
    text PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data PT_LOAD FLAGS(6);
}}

SECTIONS
{{
    . = 0x{base:08x};

    .text : {{
        KEEP(*(.text.entry))
        *(.text .text.*)
    }} :text

    . = ALIGN(0x1000);
    .rodata : {{
        *(.rodata .rodata.*)
    }} :rodata

    . = ALIGN(0x1000);
    .hk_service : {{
        KEEP(*(.hk_service))
    }} :rodata

    . = ALIGN(0x1000);
    .data : {{
        *(.data .data.*)
        *(.data.rel.ro .data.rel.ro.*)
        *(.got .got.*)
    }} :data

    . = ALIGN(0x1000);
    .bss : {{
        *(COMMON)
        *(.bss .bss.*)
    }} :data

    /DISCARD/ : {{
        *(.eh_frame)
        *(.comment)
        *(.note .note.*)
        *(.rela .rela.*)
    }}
}}
"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assigns_unique_aligned_slots() {
        let mut bases = std::collections::BTreeSet::new();
        for (index, service) in SERVERS.iter().enumerate() {
            let base = image_base(service).expect("服务已登记");
            assert_eq!(base, IMAGE_BASE + index as u64 * IMAGE_STRIDE);
            assert!(base.is_multiple_of(0x1000));
            assert!(bases.insert(base), "服务基址重复：{service}");
        }
    }

    #[test]
    fn image_region_stays_below_fixed_mappings() {
        let end = IMAGE_BASE + SERVERS.len() as u64 * IMAGE_STRIDE;
        assert!(end <= 0x3000_0000, "镜像区与网络固定映射重叠");
    }
}
