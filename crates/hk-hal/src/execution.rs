//! 用户态执行现场。
//!
//! 平台实现该 trait：恢复 ring 3、设置系统调用结果，并把 `_start` 参数写入入口
//! 约定位置。调度器与系统调用分派只依赖本 trait，不感知寄存器布局。

/// 用户态执行现场。
pub trait UserContext: Copy {
    /// 以保存的现场恢复用户态执行，返回时线程已阻塞或退出。
    fn resume(&self);

    /// 设置服务入口的首个参数。
    fn set_start_argument(&mut self, argument: u64);

    /// 返回系统调用号。
    fn number(&self) -> u64;

    /// 返回第一个参数。
    fn arg0(&self) -> u64;

    /// 返回第二个参数。
    fn arg1(&self) -> u64;

    /// 返回第三个参数。
    fn arg2(&self) -> u64;

    /// 设置系统调用返回值。
    fn set_result(&mut self, value: u64);

    /// 设置消息字。
    fn set_word(&mut self, value: u64);

    /// 设置发送者端点。
    fn set_sender(&mut self, value: u64);
}
