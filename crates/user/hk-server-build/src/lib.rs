//! 服务镜像链接配置。
//!
//! 每个服务运行在自己的用户页表中，镜像基址不需要按服务区分；这里让所有
//! 服务共享同一段用户镜像区。构建脚本调用 [`configure`] 即可，不再维护服务
//! 地址表，也不需要知道镜像里有哪些服务。

use std::path::PathBuf;

pub use hk_base::user::{IMAGE_BASE, IMAGE_LIMIT};

/// 为当前包生成链接脚本。
///
/// 所有服务使用同一基址；服务身份由镜像名与 Root 控制查询区分，不依赖地址。
pub fn configure() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "none" {
        return;
    }

    println!("cargo:rustc-env=HK_IMAGE_BASE=0x{IMAGE_BASE:x}");
    configure_at(IMAGE_BASE);
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

// 镜像区必须页对齐，并与 Arena/栈及固定映射保持距离。
const _: () = {
    assert!(IMAGE_BASE.is_multiple_of(0x1000));
    assert!(IMAGE_LIMIT > IMAGE_BASE);
    assert!(IMAGE_LIMIT <= hk_base::user::AUTO_WINDOW_BASE);
};
