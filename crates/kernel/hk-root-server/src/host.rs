//! 运行时宿主：全局持有用户运行时，提供启动、调度与系统调用入口。
//!
//! 平台只负责在锁外恢复用户现场，并在系统调用/抢占回调里把现场交给本宿主。

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use hk_cadence::CadenceSpec;
use hk_checkpoint::Page;
use hk_hal::{SyscallFrame, UserContext};
use spin::Mutex;

use hk_runtime::ServiceSpec;
use hk_runtime::runtime::{
    CheckpointCapture, FaultOutcome, Step, Switch, TaskInfo, UserPlatform, UserRuntime,
};

use crate::ServiceTable;
use crate::debug::{self, FaultContext};

/// 把用户运行时适配为调试内存访问。
struct RuntimeMemory<'a, C, P>(&'a UserRuntime<C, P>);

impl<C: UserContext + SyscallFrame, P: UserPlatform> debug::Memory for RuntimeMemory<'_, C, P> {
    fn mapped(&self, address: u64) -> bool {
        self.0.user_mapped(address)
    }

    fn read(&self, address: u64, buffer: &mut [u8]) -> bool {
        self.0.read_user(address, buffer)
    }
}

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
    pub fn boot<F>(&self, specs: &[ServiceSpec], platform: P, make_context: F) -> Result<(), String>
    where
        F: FnMut(usize, u64) -> C,
    {
        let runtime = hk_runtime::boot::user_runtime(specs, platform, make_context)
            .map_err(|error| format!("{error}"))?;
        *self.runtime.lock() = Some(runtime);
        Ok(())
    }

    /// 设置任务的用户态初始现场。
    pub fn attach(&self, index: usize, context: C) {
        if let Some(runtime) = self.runtime.lock().as_mut() {
            runtime.attach(index, context);
        }
        self.wake_aps();
    }

    /// 返回当前 CPU 正在执行的任务名。
    pub fn current_task_name(&self, cpu: usize) -> Option<String> {
        self.runtime
            .lock()
            .as_ref()
            .and_then(|runtime| runtime.current_task_name(cpu))
            .map(String::from)
    }

    /// 安装镜像符号表。
    pub fn install_symbols(&self, bytes: &[u8]) {
        debug::install_symbols(bytes);
    }

    /// 把用户异常原始现场投递给已订阅的端点。
    pub fn deliver_fault(&self, cpu: usize, context: FaultContext) {
        let task = self
            .current_task_name(cpu)
            .unwrap_or_else(|| String::from("?"));
        let mut guard = self.runtime.lock();
        if let Some(runtime) = guard.as_mut() {
            let text = {
                let memory = RuntimeMemory(runtime);
                debug::format_record(&task, context, &memory)
            };
            runtime.deliver_fault_event(text.as_bytes());
        }
    }

    /// 符号化地址列表。
    pub fn symbolize_list(&self, argument: &str) -> String {
        debug::symbolize_list(argument)
    }

    /// 从当前任务用户内存读取字节。
    pub fn read_user(&self, address: u64, buffer: &mut [u8]) -> bool {
        self.runtime
            .lock()
            .as_ref()
            .is_some_and(|runtime| runtime.read_user(address, buffer))
    }

    /// 向当前任务用户内存写入字节。
    pub fn write_user(&self, address: u64, data: &[u8]) -> bool {
        self.runtime
            .lock()
            .as_ref()
            .is_some_and(|runtime| runtime.write_user(address, data))
    }

    /// 处理匿名能力的按需零页缺页。
    pub fn anonymous_fault(&self, cpu: usize, address: u64) -> bool {
        self.runtime
            .lock()
            .as_mut()
            .is_some_and(|runtime| runtime.anonymous_fault(cpu, address))
    }

    /// 把缺页请求转给绑定 Provider；`context` 是缺页现场，任务恢复时从这里继续。
    pub fn provider_fault(&self, cpu: usize, address: u64, context: C) -> bool {
        self.runtime
            .lock()
            .as_mut()
            .is_some_and(|runtime| runtime.provider_fault(cpu, address, context))
    }

    /// 解析一次 Region 后处理缺页。
    pub fn handle_fault(&self, cpu: usize, address: u64, context: C) -> FaultOutcome {
        self.runtime
            .lock()
            .as_mut()
            .map_or(FaultOutcome::Unhandled, |runtime| {
                runtime.handle_fault(cpu, address, context)
            })
    }

    /// 返回任务数量。
    pub fn task_count(&self) -> usize {
        self.runtime
            .lock()
            .as_ref()
            .map_or(0, UserRuntime::task_count)
    }

    /// 是否有尚未到期的休眠任务。
    pub fn has_sleepers(&self) -> bool {
        self.runtime
            .lock()
            .as_ref()
            .is_some_and(|runtime| runtime.has_sleepers())
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

    /// 捕获并保存任务检查点，返回镜像字节数。
    pub fn checkpoint(&self, name: &str) -> Result<usize, String> {
        self.runtime
            .lock()
            .as_mut()
            .ok_or_else(|| String::from("运行时未初始化"))?
            .checkpoint(name)
    }

    /// 捕获一个未在 CPU 上执行的任务现场。
    pub fn capture_checkpoint(&self, name: &str) -> Result<CheckpointCapture<C>, String> {
        self.runtime
            .lock()
            .as_mut()
            .ok_or_else(|| String::from("运行时未初始化"))?
            .capture_checkpoint(name)
    }

    /// 终止任务并保存检查点镜像。
    pub fn commit_checkpoint(
        &self,
        name: &str,
        capture: CheckpointCapture<C>,
        image: Vec<u8>,
    ) -> Result<usize, String> {
        self.runtime
            .lock()
            .as_mut()
            .ok_or_else(|| String::from("运行时未初始化"))?
            .commit_checkpoint(name, capture, image)
    }

    /// 从保存的现场恢复任务。
    pub fn restore_checkpoint(&self, name: &str) -> Result<usize, String> {
        self.runtime
            .lock()
            .as_mut()
            .ok_or_else(|| String::from("运行时未初始化"))?
            .restore_checkpoint(name)
    }

    /// 从序列化镜像重建任务。
    pub fn restore_image(&self, image: &[u8]) -> Result<(String, usize), String> {
        self.runtime
            .lock()
            .as_mut()
            .ok_or_else(|| String::from("运行时未初始化"))?
            .restore_image(image)
    }

    /// 按快照重建任务。
    #[allow(clippy::too_many_arguments)]
    pub fn restore_parts(
        &self,
        name: &str,
        slot: usize,
        context: C,
        parent: Option<u64>,
        arena_used: u64,
        cadence: hk_cadence::CadenceSnapshot,
        meter: hk_cadence::MeterSnapshot,
        pages: &[Page],
    ) -> Result<usize, String> {
        self.runtime
            .lock()
            .as_mut()
            .ok_or_else(|| String::from("运行时未初始化"))?
            .restore_parts(
                name, slot, context, parent, arena_used, cadence, meter, pages,
            )
    }

    /// 读取快照镜像片段。
    pub fn checkpoint_chunk(&self, name: &str, offset: u64, length: usize) -> Option<Vec<u8>> {
        self.runtime
            .lock()
            .as_ref()?
            .checkpoint_chunk(name, offset, length)
    }

    /// 终止一个未在 CPU 上执行的任务。
    pub fn terminate(&self, name: &str) -> bool {
        self.runtime
            .lock()
            .as_mut()
            .is_some_and(|runtime| runtime.terminate(name, 1))
    }

    /// 装载新任务，返回分配的端点编号。
    pub fn start(
        &self,
        name: &str,
        device_authority: bool,
        cadence: CadenceSpec,
        context: C,
        parent: Option<u64>,
    ) -> Result<u64, String> {
        let result = {
            let mut guard = self.runtime.lock();
            let runtime = guard
                .as_mut()
                .ok_or_else(|| String::from("运行时未初始化"))?;
            runtime
                .start(name, device_authority, cadence, context, parent)
                .map_err(|error| format!("{error}"))
        };
        self.wake_aps();
        result
    }

    /// 授予具名服务的调度策略授权。
    pub fn grant_policy_authority(&self, name: &str) -> bool {
        self.runtime
            .lock()
            .as_mut()
            .is_some_and(|runtime| runtime.grant_policy_authority(name))
    }

    /// 输出任务的 Cadence 契约。
    pub fn cadence_text(&self, name: &str) -> String {
        self.runtime.lock().as_ref().map_or_else(
            || String::from("运行时未初始化\n"),
            |runtime| runtime.cadence_text(name),
        )
    }

    /// 输出任务的 Meter 计量。
    pub fn meter_text(&self, name: &str) -> String {
        self.runtime.lock().as_ref().map_or_else(
            || String::from("运行时未初始化\n"),
            |runtime| runtime.meter_text(name),
        )
    }

    /// 输出 Tempo 策略池与自身计量。
    pub fn tempo_text(&self) -> String {
        self.runtime.lock().as_ref().map_or_else(
            || String::from("运行时未初始化\n"),
            |runtime| runtime.tempo_text(),
        )
    }

    /// 注册跨核抢占回调。
    pub fn set_resched_hook(&self, hook: fn(usize)) {
        if let Some(runtime) = self.runtime.lock().as_ref() {
            runtime.set_resched_hook(hook);
        }
    }

    /// 设置调度器可见的在线 CPU 数。
    pub fn set_online_cpus(&self, online: usize) {
        if let Some(runtime) = self.runtime.lock().as_ref() {
            runtime.set_online_cpus(online);
        }
    }

    /// 为每个在线 CPU 建立 idle TCB。
    pub fn enable_idle_cpus(&self, online: usize) {
        if let Some(runtime) = self.runtime.lock().as_ref() {
            runtime.enable_idle_cpus(online);
        }
    }

    /// 生成任务视图快照。
    pub fn tasks(&self) -> Vec<TaskInfo> {
        self.runtime
            .lock()
            .as_ref()
            .map_or_else(Vec::new, UserRuntime::tasks)
    }

    /// 返回最近的内核 trace 事件。
    pub fn trace(&self, limit: usize) -> Vec<hk_kernel::TraceEvent> {
        self.runtime
            .lock()
            .as_ref()
            .map_or_else(Vec::new, |runtime| runtime.trace(limit))
    }

    /// 返回运行时统计。
    pub fn stats(&self) -> crate::RuntimeStats {
        self.runtime
            .lock()
            .as_ref()
            .map_or_else(crate::RuntimeStats::default, UserRuntime::stats)
    }

    /// 在 BSP 上依次调度任务直到没有可运行线程；恢复现场时释放锁。
    pub fn run(&self, table: &mut ServiceTable) {
        self.run_cpu(0, Some(table));
    }

    /// 在指定 CPU 上调度任务；`table` 为 `None` 时退出记录留待 BSP 同步。
    pub fn run_cpu(&self, cpu: usize, table: Option<&mut ServiceTable>) {
        self.run_cpu_budget(cpu, table, usize::MAX);
    }

    /// 在指定 CPU 上调度至多 `budget` 步；返回是否已无可运行线程。
    pub fn run_cpu_budget(
        &self,
        cpu: usize,
        mut table: Option<&mut ServiceTable>,
        budget: usize,
    ) -> bool {
        let mut steps = 0;
        loop {
            if steps >= budget {
                return false;
            }
            steps += 1;
            let step = self.runtime.lock().as_mut().map(|runtime| {
                let step = runtime.step(cpu);
                runtime.arm_wakeup(cpu);
                step
            });
            match step {
                Some(Step::Resume { index, context }) => {
                    context.resume();
                    let exited = self
                        .runtime
                        .lock()
                        .as_mut()
                        .and_then(|runtime| runtime.finish(cpu, index));
                    if let Some((name, code)) = exited {
                        match table.as_deref_mut() {
                            Some(table) => table.mark_exited(&name, code),
                            None => {
                                if let Some(runtime) = self.runtime.lock().as_mut() {
                                    runtime.defer_exit(name, code);
                                }
                            }
                        }
                    }
                }
                Some(Step::Blocked) => {}
                Some(Step::Done) | None => return true,
            }
        }
    }

    /// AP 调度循环：无任务时轮询等待新任务。
    pub fn run_ap(&self, cpu: usize) -> ! {
        loop {
            self.run_cpu(cpu, None);
            core::hint::spin_loop();
        }
    }

    /// 把其他 CPU 记录的服务退出同步到服务表。
    pub fn drain_exits(&self, table: &mut ServiceTable) {
        let exits = self
            .runtime
            .lock()
            .as_mut()
            .map(|runtime| runtime.take_exits())
            .unwrap_or_default();
        for (name, code) in exits {
            table.mark_exited(&name, code);
        }
    }

    /// 分派一次系统调用。
    pub fn dispatch(&self, cpu: usize, frame: &mut C) -> Switch {
        let mut guard = self.runtime.lock();
        let Some(runtime) = guard.as_mut() else {
            frame.set_result(u64::MAX);
            return Switch::Continue;
        };
        let switch = runtime.dispatch(cpu, frame);
        if runtime.has_runnable() {
            runtime.wake_aps();
        }
        switch
    }

    /// 有可运行线程时唤醒空闲 CPU。
    fn wake_aps(&self) {
        if let Some(runtime) = self.runtime.lock().as_ref()
            && runtime.has_runnable()
        {
            runtime.wake_aps();
        }
    }

    /// 保存抢占现场；返回是否切换任务。
    pub fn preempt(&self, cpu: usize, context: C) -> bool {
        self.runtime
            .lock()
            .as_mut()
            .is_some_and(|runtime| runtime.preempt(cpu, context))
    }

    /// 标记当前任务因用户态异常退出。
    pub fn fault(&self, cpu: usize, code: u64) -> bool {
        self.runtime
            .lock()
            .as_mut()
            .is_some_and(|runtime| runtime.fault(cpu, code))
    }
}
