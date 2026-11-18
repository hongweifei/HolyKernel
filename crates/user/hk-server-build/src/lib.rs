//! 服务镜像链接配置。
//!
//! 所有服务共享同一段用户镜像区；链接脚本由 xtask 统一生成并通过
//! `RUSTFLAGS` 注入，服务二进制不再各自维护 `build.rs`。

pub use hk_base::user::{IMAGE_BASE, IMAGE_LIMIT};

/// 生成链接脚本文本。
pub fn linker_script(base: u64) -> String {
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

/// 把链接脚本写入 `path`。
pub fn write_linker_script(path: &std::path::Path, base: u64) -> std::io::Result<()> {
    std::fs::write(path, linker_script(base))
}

// 镜像区必须页对齐，并与 Arena/栈及固定映射保持距离。
const _: () = {
    assert!(IMAGE_BASE.is_multiple_of(0x1000));
    assert!(IMAGE_LIMIT > IMAGE_BASE);
    assert!(IMAGE_LIMIT <= hk_base::user::AUTO_WINDOW_BASE);
};
