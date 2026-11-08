//! Console 自检：Display 热重启后的重连探测。

/// Display 重启自检状态。
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Restart {
    enabled: bool,
    reconnected: bool,
}

impl Restart {
    /// 从启动命令行解析自检是否启用。
    pub(crate) fn new(cmdline: Option<&str>) -> Self {
        Self {
            enabled: cmdline.is_some_and(|line| line.contains("display-restart-test")),
            reconnected: false,
        }
    }

    /// 是否已经重新连上新 Display。
    pub(crate) fn reconnected(self) -> bool {
        self.reconnected
    }

    /// 记录一次成功重连。
    pub(crate) fn mark_reconnected(&mut self) {
        self.reconnected = true;
    }

    /// 自检完成条件：启用、已重连且 Display 已退出。
    pub(crate) fn finished(self, current_display: u64) -> bool {
        self.enabled && self.reconnected && current_display == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_for_reconnect_then_display_exit() {
        let mut restart = Restart::new(Some("display-restart-test"));
        assert!(!restart.finished(0));
        restart.mark_reconnected();
        assert!(restart.reconnected());
        assert!(!restart.finished(1));
        assert!(restart.finished(0));
    }

    #[test]
    fn disabled_without_flag() {
        let restart = Restart::new(None);
        assert!(!restart.finished(0));
    }
}
