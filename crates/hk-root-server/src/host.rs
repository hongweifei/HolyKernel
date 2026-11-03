//! 运行时宿主：全局持有用户运行时，提供启动、调度与系统调用入口。
//!
//! 平台只负责在锁外恢复用户现场，并在系统调用/抢占回调里把现场交给本宿主。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use hk_hal::{SyscallFrame, UserContext};
use spin::Mutex;

use crate::ServiceTable;
use crate::runtime::{Step, Switch, TaskInfo, UserPlatform, UserRuntime};

/// 运行时宿主。
pub struct RuntimeHost<C, P> {
    runtime: Mutex<Option<UserRuntime<C, P>>>,
}

impl<C, P> Default for RuntimeHost<C, P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C, P> RuntimeHost<C, P> {
    /// 构造空宿主。
    pub const fn new() -> Self {
        Self {
            runtime: Mutex::new(None),
        }
    }
}

impl<C: UserContext + SyscallFrame, P: UserPlatform> RuntimeHost<C, P> {
    /// 按服务清单启动运行时。
    pub fn boot<F>(
        &self,
        names: &[&'static str],
        platform: P,
        make_context: F,
    ) -> Result<(), String>
    where
        F: FnMut(usize, u64) -> C,
    {
        let runtime = crate::boot::user_runtime(names, platform, make_context)
            .map_err(|error| format!("{error}"))?;
        *self.runtime.lock() = Some(runtime);
        Ok(())
    }

    /// 设置任务的用户态初始现场。
    pub fn attach(&self, index: usize, context: C) {
        if let Some(runtime) = self.runtime.lock().as_mut() {
            runtime.attach(index, context);
        }
    }

    /// 返回任务数量。
    pub fn task_count(&self) -> usize {
        self.runtime
            .lock()
            .as_ref()
            .map_or(0, UserRuntime::task_count)
    }

    /// 按名称返回任务的 TCB 编号。
    pub fn tcb_of(&self, name: &str) -> Option<u64> {
        self.runtime
            .lock()
            .as_ref()
            .and_then(|runtime| runtime.tcb_of(name))
    }

    /// 返回 Root 线程编号。
    pub fn root(&self) -> u64 {
        self.runtime.lock().as_ref().map_or(0, UserRuntime::root)
    }

    /// 装载新任务，返回分配的端点编号。
    pub fn start(&self, name: &str, context: C, parent: Option<u64>) -> Result<u64, String> {
        let mut guard = self.runtime.lock();
        let runtime = guard
            .as_mut()
            .ok_or_else(|| String::from("运行时未初始化"))?;
        runtime
            .start(name, context, parent)
            .map_err(|error| format!("{error}"))
    }

    /// 生成任务视图快照。
    pub fn tasks(&self) -> Vec<TaskInfo> {
        self.runtime
            .lock()
            .as_ref()
            .map_or_else(Vec::new, UserRuntime::tasks)
    }

    /// 依次调度任务直到没有可运行线程；恢复现场时释放锁。
    pub fn run(&self, table: &mut ServiceTable) {
        loop {
            let step = self.runtime.lock().as_mut().map(UserRuntime::step);
            match step {
                Some(Step::Resume { index, context }) => {
                    context.resume();
                    let exited = self
                        .runtime
                        .lock()
                        .as_mut()
                        .and_then(|runtime| runtime.finish(index));
                    if let Some((name, code)) = exited {
                        table.mark_exited(&name, code);
                    }
                }
                Some(Step::Blocked) => {}
                Some(Step::Done) | None => break,
            }
        }
    }

    /// 分派一次系统调用。
    pub fn dispatch(&self, frame: &mut C) -> Switch {
        let mut guard = self.runtime.lock();
        let Some(runtime) = guard.as_mut() else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        runtime.dispatch(frame)
    }

    /// 保存抢占现场；返回是否切换任务。
    pub fn preempt(&self, context: C) -> bool {
        self.runtime
            .lock()
            .as_mut()
            .is_some_and(|runtime| runtime.preempt(context))
    }
}
