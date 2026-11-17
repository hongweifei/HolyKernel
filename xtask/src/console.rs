//! 子进程写控制台时的代码页。
//!
//! xtask 自己的 `println!` 走 `WriteConsoleW`，代码页与中文无关；QEMU 不同：
//! 它把串口字节用 C 运行时直接写到控制台句柄，控制台按当前输出代码页解释。
//! 简体中文 Windows 的控制台默认是 936（GBK），于是 `cargo xtask run` 里内核与
//! 服务打印的 UTF-8 中文全部变成乱码。启动 QEMU 前把控制台代码页切到 65001，
//! 退出后恢复原值。

/// 控制台代码页守卫：构造时切到 UTF-8，析构时恢复构造前的代码页。
#[derive(Debug)]
pub struct Utf8Guard {
    #[cfg(windows)]
    previous: Option<(u32, u32)>,
}

/// 把控制台 I/O 代码页切换到 UTF-8，返回的守卫离开作用域时恢复。
///
/// 非 Windows 平台或输出被重定向而没有控制台时为空操作。
pub fn utf8() -> Utf8Guard {
    #[cfg(windows)]
    {
        // SAFETY: 只查询当前进程控制台的代码页，没有控制台时返回 0。
        let previous = unsafe { (GetConsoleOutputCP(), GetConsoleCP()) };
        let previous = if previous.0 == 0 {
            None
        } else {
            // SAFETY: 65001 是合法代码页；查询成功说明控制台存在。
            unsafe {
                SetConsoleOutputCP(CP_UTF8);
                SetConsoleCP(CP_UTF8);
            }
            Some(previous)
        };
        Utf8Guard { previous }
    }
    #[cfg(not(windows))]
    Utf8Guard {}
}

impl Drop for Utf8Guard {
    fn drop(&mut self) {
        #[cfg(windows)]
        if let Some((output, input)) = self.previous {
            // SAFETY: 恢复构造时读到的代码页；控制台可能已经关闭，失败无需处理。
            unsafe {
                SetConsoleOutputCP(output);
                SetConsoleCP(input);
            }
        }
    }
}

#[cfg(windows)]
const CP_UTF8: u32 = 65001;

#[cfg(windows)]
// SAFETY: `kernel32` 随进程加载，签名与 Win32 文档一致。
unsafe extern "system" {
    /// `GetConsoleOutputCP`：读取控制台输出代码页。
    fn GetConsoleOutputCP() -> u32;
    /// `GetConsoleCP`：读取控制台输入代码页。
    fn GetConsoleCP() -> u32;
    /// `SetConsoleOutputCP`：设置控制台输出代码页。
    fn SetConsoleOutputCP(code_page: u32) -> i32;
    /// `SetConsoleCP`：设置控制台输入代码页。
    fn SetConsoleCP(code_page: u32) -> i32;
}
