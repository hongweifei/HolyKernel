//! 用户态执行现场与系统调用帧。

/// 用户态执行现场。
pub trait UserContext: Copy {
    /// 以保存的现场恢复用户态执行，返回时线程已阻塞或退出。
    fn resume(&self);

    /// 设置服务入口的首个参数。
    fn set_start_argument(&mut self, argument: u64);

    /// 构造同地址空间新线程的入口现场。
    fn for_thread(entry: u64, stack_top: u64, argument: u64) -> Self;
}

/// 系统调用帧的寄存器视图。
pub trait SyscallFrame: Copy {
    /// 返回系统调用号。
    fn number(&self) -> u64;

    /// 返回第一个参数。
    fn arg0(&self) -> u64;

    /// 返回第二个参数。
    fn arg1(&self) -> u64;

    /// 返回第三个参数。
    fn arg2(&self) -> u64;

    /// 返回第四个参数。
    fn arg3(&self) -> u64;

    /// 设置系统调用返回值。
    fn set_result(&mut self, value: u64);

    /// 设置消息字。
    fn set_word(&mut self, value: u64);

    /// 设置发送者端点。
    fn set_sender(&mut self, value: u64);

    /// 设置随消息接收的句柄编号；`0` 表示没有句柄。
    fn set_handle(&mut self, value: u64);
}
