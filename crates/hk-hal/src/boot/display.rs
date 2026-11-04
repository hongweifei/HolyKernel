//! 启动信息打印。

use core::fmt;

use super::*;

impl fmt::Display for BootInfo {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(formatter, "  架构: {}", self.arch)?;
        writeln!(formatter, "  内存 map:")?;
        for region in self.memory_map {
            writeln!(
                formatter,
                "    {:#012x} + {:#010x}  {}",
                region.base.get(),
                region.len,
                region.kind.name()
            )?;
        }
        writeln!(
            formatter,
            "  内核镜像: {:#012x} + {:#010x}",
            self.kernel_image.base.get(),
            self.kernel_image.len
        )?;
        match self.framebuffer {
            Some(framebuffer) => writeln!(
                formatter,
                "  帧缓冲: {:#012x} {}x{} stride={}",
                framebuffer.phys.get(),
                framebuffer.width,
                framebuffer.height,
                framebuffer.stride
            )?,
            None => writeln!(formatter, "  帧缓冲: 无")?,
        }
        match self.direct_map {
            Some(direct_map) => writeln!(
                formatter,
                "  direct map: {:#012x} -> {:#018x} + {:#010x}",
                direct_map.phys_base.get(),
                direct_map.virt_base.get(),
                direct_map.len
            )?,
            None => writeln!(formatter, "  direct map: 无")?,
        }
        match self.rsdp {
            Some(rsdp) => writeln!(formatter, "  RSDP: {:#012x}", rsdp.get())?,
            None => writeln!(formatter, "  RSDP: 无")?,
        }
        writeln!(formatter, "  服务器模块: {}", self.server_modules.len())
    }
}
