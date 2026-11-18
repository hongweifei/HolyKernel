//! 启动模块只读查询；仅 Image Server 可访问。

use crate::runtime::{Switch, UserPlatform, UserRuntime};
use hk_hal::{SyscallFrame, UserContext};

impl<C: UserContext + SyscallFrame, P: UserPlatform> UserRuntime<C, P> {
    fn module_reader(&self, index: usize) -> bool {
        self.tasks[index].name == "hk-server-image"
    }

    pub(crate) fn service_watch(&mut self, index: usize, frame: &mut C) -> Switch {
        let address = frame.arg0();
        let length = frame.arg1() as usize;
        let number = frame.arg2() as usize;
        if length == 0 || length > 64 {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let mut name = alloc::vec![0_u8; length];
        if !self.platform.read_user(address, &mut name) {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let Ok(name) = alloc::string::String::from_utf8(name) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let Some(handle) = self.notification_handle(index, number) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let tcb = self.tasks[index].tcb;
        let Ok(id) = self.kernel.handle_id(tcb, handle) else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        self.service_watchers.push((name, id));
        frame.set_result(0);
        Switch::Continue
    }

    pub(crate) fn module_count(&self, index: usize, frame: &mut C) -> Switch {
        frame.set_result(if self.module_reader(index) {
            self.modules.len() as u64
        } else {
            u64::MAX
        });
        Switch::Continue
    }

    pub(crate) fn module_info(&self, index: usize, frame: &mut C) -> Switch {
        if !self.module_reader(index) {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let module_index = frame.arg0() as usize;
        let out = frame.arg1();
        let capacity = frame.arg2() as usize;
        let Some(module) = self.modules.get(module_index) else {
            frame.set_result(0);
            return Switch::Continue;
        };
        if capacity < hk_syscall::MODULE_INFO_BYTES {
            frame.set_result(0);
            return Switch::Continue;
        }
        let name = module.name.as_bytes();
        let mut bytes = alloc::vec![0_u8; hk_syscall::MODULE_INFO_BYTES + name.len()];
        bytes[..8].copy_from_slice(&module.range.len().to_le_bytes());
        bytes[8..16].copy_from_slice(&(name.len() as u64).to_le_bytes());
        bytes[hk_syscall::MODULE_INFO_BYTES..].copy_from_slice(name);
        let written = bytes.len().min(capacity);
        frame.set_result(if self.platform.write_user(out, &bytes[..written]) {
            written as u64
        } else {
            u64::MAX
        });
        Switch::Continue
    }

    pub(crate) fn module_read(&self, index: usize, frame: &mut C) -> Switch {
        if !self.module_reader(index) {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        let module_index = frame.arg0() as usize;
        let offset = frame.arg1();
        let out = frame.arg2();
        let length = frame.arg3() as usize;
        let Some(module) = self.modules.get(module_index) else {
            frame.set_result(0);
            return Switch::Continue;
        };
        let size = module.range.len();
        if offset >= size || length == 0 {
            frame.set_result(0);
            return Switch::Continue;
        }
        let length = length.min((size - offset) as usize);
        let mut bytes = alloc::vec![0_u8; length];
        if !self.platform.read_phys(
            hk_hal::PhysAddr::new(module.range.start().get().saturating_add(offset)),
            &mut bytes,
        ) {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        }
        frame.set_result(if self.platform.write_user(out, &bytes) {
            length as u64
        } else {
            u64::MAX
        });
        Switch::Continue
    }
}
