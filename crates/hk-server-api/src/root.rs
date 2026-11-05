//! Root Server 控制客户端。
//!
//! 用户态只发送操作码和参数，由内核 Root Server 返回文本结果；本模块不依赖
//! 运行时、装载器或服务表实现。

use crate::{syscall_call, yield_now};

pub use hk_syscall::{
    ROOT_OP_FAULT, ROOT_OP_HANDLES, ROOT_OP_REPLACE, ROOT_OP_RESTART, ROOT_OP_SCRIPT,
    ROOT_OP_SERVICES, ROOT_OP_START, ROOT_OP_STATS, ROOT_OP_SYMBOLIZE, ROOT_OP_TASKS,
    ROOT_OP_TRACE, SYS_ROOT_CONTROL, SYS_ROOT_RESULT,
};

/// 控制结果未就绪。
pub const PENDING: u64 = u64::MAX;

/// 发起控制请求并等待文本结果。
///
/// 成功返回写入 `out` 的字节数；失败返回错误码。结果只保证是 UTF-8 文本，
/// 具体格式由 Root Server 决定。
pub fn call(op: u64, argument: &str, out: &mut [u8]) -> Result<usize, u64> {
    // SAFETY: 参数来自本任务只读内存；内核校验后复制。
    let id = unsafe {
        syscall_call(
            SYS_ROOT_CONTROL,
            op,
            argument.as_ptr() as u64,
            argument.len() as u64,
            0,
        )
        .value
    };
    if id == u64::MAX {
        return Err(1);
    }

    loop {
        // SAFETY: 输出缓冲区属于本任务；内核只写入其范围。
        let result = unsafe {
            syscall_call(
                SYS_ROOT_RESULT,
                id,
                out.as_mut_ptr() as u64,
                out.len() as u64,
                0,
            )
            .value
        };
        if result != PENDING {
            return if result == u64::MAX {
                Err(2)
            } else {
                Ok(result as usize)
            };
        }
        let _ = yield_now();
    }
}
