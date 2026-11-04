//! 服务镜像链接配置。
//!
//! 每个服务固定链接到独立基址，生成与 demo 相同的链接脚本，避免逐个维护
//! 相同的 `linker.ld`。

use std::path::PathBuf;

/// 为服务镜像生成链接脚本并设置链接参数。
pub fn configure(base: u64) {
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
