//! 挂起状态与调度步骤。

use super::*;

/// 缺页处理结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultOutcome {
    /// 已转交 Provider，任务阻塞。
    Blocked,
    /// 页已装入或恢复映射，任务可继续。
    Resumed,
    /// 当前 Region 不处理该缺页。
    Unhandled,
}

/// 被挂起的系统调用。
#[derive(Clone, Copy)]
pub enum Pending {
    /// 没有挂起操作。
    None,
    /// 发送已排队，等待接收者取走。
    Send {
        /// 目标端点编号。
        endpoint: u64,
        /// 发送使用的端点句柄。
        handle: Handle,
    },
    /// 接收已登记，等待消息到达。
    Recv {
        /// 接收使用的端点句柄。
        handle: Handle,
    },
    /// 字节接收已登记。
    RecvData {
        /// 接收使用的端点句柄。
        handle: Handle,
        /// 用户缓冲区地址。
        buffer: u64,
        /// 缓冲区容量。
        capacity: usize,
        /// 期望的发送线程。
        expected: Option<TcbId>,
        /// 是否把随消息收到的句柄编号打包返回。
        handle_out: bool,
        /// 超时 deadline；`None` 表示无限等待。
        deadline: Option<u64>,
    },
    /// 通知等待已登记。
    Notify {
        /// 通知句柄。
        handle: Handle,
        /// 等待的通知位。
        mask: u64,
    },
    /// 调用已发出，等待服务回复。
    Call {
        /// 调用目标端点编号，用于回报发送者。
        endpoint: u64,
        /// 调用开始时间戳。
        started: u64,
        /// 超时 deadline；`None` 表示无限等待。
        deadline: Option<u64>,
    },
    /// 缺页请求已发给 Provider，等待物理页回包。
    ProviderFault {
        /// 目标能力句柄。
        capability: Handle,
        /// 目标能力内的页下标。
        index: usize,
        /// 待安装页的用户虚拟地址。
        page: u64,
        /// 该页所属 Region 的映射权利。
        rights: Rights,
        /// 该页所属 Region 的映射标志。
        flags: RegionFlags,
        /// 是否为写时复制的私有页；Provider 回包安装到进程覆盖而不是能力。
        private: bool,
    },
    /// 休眠到指定契约时刻（微秒）。
    Sleep {
        /// 唤醒时刻（契约 tick）。
        wake: u64,
    },
}

/// 系统调用后的调度动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Switch {
    /// 返回用户态。
    Continue,
    /// 保存现场并切换到下一个任务。
    Block,
    /// 任务退出。
    Exit,
}

/// 调度一步的结果。
pub enum Step<C> {
    /// 恢复该任务的用户态现场。
    Resume {
        /// 任务下标，供 [`UserRuntime::finish`] 收尾。
        index: usize,
        /// 用户态现场。
        context: C,
    },
    /// 当前任务阻塞，继续调度下一个。
    Blocked,
    /// 这一刻没有可被选中的任务。
    ///
    /// 任务可能只是在等预算补充、sleep 到期或外部唤醒；调用方按下一次唤醒
    /// 期限与任务状态决定继续等待还是收尾。
    Done,
}
