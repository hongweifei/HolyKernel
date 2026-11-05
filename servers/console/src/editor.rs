//! 行编辑、回显与按行交付。

use hk_server_api::console::CONSOLE_READ_LINE;
use hk_server_api::{send_data, write};

use crate::terminal::Terminal;

/// 输出到图形终端；没有终端时退回内核控制台。
pub(crate) fn emit(terminal: &mut Option<Terminal>, text: &[u8]) {
    match terminal {
        Some(terminal) => terminal.emit(text),
        None => write(text),
    }
}

/// 行编辑状态。
pub(crate) struct Editor {
    line: [u8; 128],
    length: usize,
    pending: u64,
    queued: [u8; 128],
    queued_length: usize,
}

impl Editor {
    /// 构造空编辑器。
    pub(crate) const fn new() -> Self {
        Self {
            line: [0; 128],
            length: 0,
            pending: 0,
            queued: [0; 128],
            queued_length: 0,
        }
    }

    /// 处理一个输入字节：行编辑、回显与按行回复。
    pub(crate) fn handle(&mut self, terminal: &mut Option<Terminal>, byte: u8) {
        match byte {
            b'\n' | b'\r' => {
                emit(terminal, b"\n");
                if self.pending != 0 {
                    let _ = send_data(self.pending, CONSOLE_READ_LINE, &self.line[..self.length]);
                    self.pending = 0;
                } else {
                    self.queued[..self.length].copy_from_slice(&self.line[..self.length]);
                    self.queued_length = self.length;
                }
                self.length = 0;
            }
            0x08 | 0x7F => {
                if self.length > 0 {
                    self.length -= 1;
                    emit(terminal, b"\x08 \x08");
                }
            }
            byte if byte >= 0x20 && byte != 0x7F && self.length < self.line.len() => {
                self.line[self.length] = byte;
                self.length += 1;
                emit(terminal, &[byte]);
            }
            _ => {}
        }
    }

    /// 处理读行请求：有缓存行立即交付，否则登记等待。
    pub(crate) fn request(&mut self, sender: u64) {
        if self.queued_length > 0 {
            let _ = send_data(
                sender,
                CONSOLE_READ_LINE,
                &self.queued[..self.queued_length],
            );
            self.queued_length = 0;
        } else {
            self.pending = sender;
        }
    }
}
