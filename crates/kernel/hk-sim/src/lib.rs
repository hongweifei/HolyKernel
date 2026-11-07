//! 确定性仿真。
//!
//! 内核只维护可运行队列与阻塞状态；线程执行体由仿真器驱动。Hosted 目标用同一
//! 份内核代码跑真实 IPC 路径，QEMU 目标在 M2 用同样的对象与消息语义执行 ELF。

#![no_std]

extern crate alloc;

#[cfg(test)]
extern crate std;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use hk_kernel::{Kernel, KernelError, TcbId};

/// 线程执行体：接收内核与自身编号，返回本步结果。
pub type ThreadBody = Box<dyn FnMut(&Kernel, TcbId) -> ThreadStep>;

/// 线程执行体的一步结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadStep {
    /// 让出 CPU，由调度器选择下一个线程。
    Continue,
    /// 线程结束。
    Exit,
}

/// 一次仿真的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunReport {
    /// 调度步数。
    pub steps: u64,
    /// 正常退出的线程数。
    pub exits: u64,
    /// trace 事件数。
    pub trace_events: usize,
    /// 随机种子。
    pub seed: u64,
}

/// 仿真错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SimError {
    /// 内核操作失败。
    Kernel(KernelError),
    /// 仍有线程阻塞且没有可运行线程。
    Deadlock {
        /// 阻塞线程数。
        blocked: usize,
    },
    /// 超过步数上限，运行未收敛。
    StepLimit {
        /// 已执行步数。
        steps: u64,
    },
}

impl fmt::Display for SimError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Kernel(error) => write!(formatter, "内核错误：{error}"),
            Self::Deadlock { blocked } => write!(formatter, "仿真死锁：{blocked} 个线程阻塞"),
            Self::StepLimit { steps } => write!(formatter, "仿真步数超过上限：{steps}"),
        }
    }
}

impl core::error::Error for SimError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Kernel(error) => Some(error),
            _ => None,
        }
    }
}

impl From<KernelError> for SimError {
    fn from(error: KernelError) -> Self {
        Self::Kernel(error)
    }
}

/// 默认步数上限。
pub const DEFAULT_STEP_LIMIT: u64 = 10_000;

/// 确定性仿真器。
pub struct Simulator {
    kernel: Kernel,
    bodies: BTreeMap<TcbId, ThreadBody>,
    steps: u64,
    step_limit: u64,
    seed: u64,
}

impl Simulator {
    /// 以内核与随机种子构造仿真器。
    pub fn new(kernel: Kernel, seed: u64) -> Self {
        Self {
            kernel,
            bodies: BTreeMap::new(),
            steps: 0,
            step_limit: DEFAULT_STEP_LIMIT,
            seed,
        }
    }

    /// 设置步数上限。
    pub fn with_step_limit(mut self, step_limit: u64) -> Self {
        self.step_limit = step_limit;
        self
    }

    /// 返回内核引用。
    pub fn kernel(&self) -> &Kernel {
        &self.kernel
    }

    /// 注册线程执行体。
    pub fn register(&mut self, thread: TcbId, body: ThreadBody) {
        self.bodies.insert(thread, body);
    }

    /// 运行到全部线程退出。
    ///
    /// 调度顺序由内核的固定优先级与 FIFO 决定，同一个种子与同一组执行体产生
    /// 相同步数与相同事件序列。
    pub fn run(&mut self) -> Result<RunReport, SimError> {
        let mut exits = 0;

        while self.kernel.thread_count() > 0 {
            let Some(thread) = self.kernel.schedule(0) else {
                return Err(SimError::Deadlock {
                    blocked: self.kernel.thread_count(),
                });
            };

            self.steps += 1;
            if self.steps > self.step_limit {
                return Err(SimError::StepLimit { steps: self.steps });
            }

            let body = self
                .bodies
                .get_mut(&thread)
                .expect("线程缺少执行体：启动流程未注册该线程");
            let step = body(&self.kernel, thread);

            if step == ThreadStep::Exit {
                self.kernel.exit(thread)?;
                self.bodies.remove(&thread);
                exits += 1;
            }
        }

        Ok(RunReport {
            steps: self.steps,
            exits,
            trace_events: self.kernel.trace_len(),
            seed: self.seed,
        })
    }

    /// 返回内核状态报告。
    pub fn dump(&self) -> String {
        self.kernel.dump()
    }

    /// 返回已注册线程编号。
    pub fn threads(&self) -> Vec<TcbId> {
        self.bodies.keys().copied().collect()
    }
}

#[cfg(test)]
mod tests;
