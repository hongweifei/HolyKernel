//! 命令执行结果。

use alloc::string::String;

/// 命令执行结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    /// 需要输出的文本。
    pub output: String,
    /// 是否清屏。
    pub clear: bool,
    /// 是否退出。
    pub exit: bool,
    /// 需要装载并启动的服务。
    pub start: Option<String>,
    /// 需要重启的服务。
    pub restart: Option<String>,
    /// 需要求值的脚本。
    pub script: Option<String>,
    /// 是否需要输出 trace。
    pub trace: bool,
    /// 是否需要输出统计。
    pub stats: bool,
}

impl Response {
    pub(crate) const fn empty() -> Self {
        Self {
            output: String::new(),
            clear: false,
            exit: false,
            start: None,
            restart: None,
            script: None,
            trace: false,
            stats: false,
        }
    }

    pub(crate) fn output(text: String) -> Self {
        Self {
            output: text,
            ..Self::empty()
        }
    }

    pub(crate) fn clear() -> Self {
        Self {
            clear: true,
            ..Self::empty()
        }
    }

    pub(crate) fn exit() -> Self {
        Self {
            exit: true,
            ..Self::empty()
        }
    }
}
