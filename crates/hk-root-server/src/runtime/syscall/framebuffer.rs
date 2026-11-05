use super::*;

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    /// 查询帧缓冲描述；只有显示服务可以调用。
    pub(crate) fn framebuffer_info(&self, index: usize, frame: &C) -> u64 {
        if self.tasks[index].name != hk_syscall::DISPLAY_SERVICE {
            return 0;
        }
        let Some(framebuffer) = self.platform.framebuffer() else {
            return 0;
        };
        let capacity = frame.arg1() as usize;
        if capacity < hk_syscall::FRAMEBUFFER_DESCRIPTOR_BYTES {
            return 0;
        }
        let descriptor = hk_syscall::FramebufferDescriptor {
            magic: hk_syscall::FRAMEBUFFER_MAGIC,
            format: framebuffer.format.code(),
            width: framebuffer.width,
            height: framebuffer.height,
            stride: framebuffer.stride,
            reserved: 0,
            phys: framebuffer.phys.get(),
            length: u64::from(framebuffer.stride) * u64::from(framebuffer.height),
        };
        let mut bytes = [0_u8; hk_syscall::FRAMEBUFFER_DESCRIPTOR_BYTES];
        descriptor.encode(&mut bytes);
        if self.platform.write_user(frame.arg0(), &bytes) {
            hk_syscall::FRAMEBUFFER_DESCRIPTOR_BYTES as u64
        } else {
            0
        }
    }

    /// 把帧缓冲映射到显示服务的地址空间，返回映射长度。
    pub(crate) fn framebuffer_map(&self, index: usize, frame: &C) -> u64 {
        if self.tasks[index].name != hk_syscall::DISPLAY_SERVICE {
            return 0;
        }
        self.platform
            .map_framebuffer(index, frame.arg0())
            .unwrap_or(0)
    }
}
