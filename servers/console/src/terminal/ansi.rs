//! ANSI 转义序列解析：CSI 光标/擦除与 SGR。

/// 解析结果。
pub(crate) enum Event {
    /// 普通字节，交给终端处理。
    Byte(u8),
    /// 一条 CSI 命令。
    Command(Command),
    /// 已被转义序列消费。
    Ignore,
}

/// CSI 命令。
pub(crate) enum Command {
    /// 设置字符属性。
    Sgr { params: [u16; 16], count: usize },
    /// 光标定位，行列从 1 开始。
    CursorPosition { row: u16, column: u16 },
    /// 上移。
    CursorUp(u16),
    /// 下移。
    CursorDown(u16),
    /// 右移。
    CursorForward(u16),
    /// 左移。
    CursorBack(u16),
    /// 列定位。
    CursorColumn(u16),
    /// 行定位。
    CursorRow(u16),
    /// 擦除显示区域。
    EraseDisplay(u16),
    /// 擦除行。
    EraseLine(u16),
}

/// 转义序列状态机。
pub(crate) struct Ansi {
    state: u8,
    params: [u16; 16],
    count: usize,
    value: u16,
    active: bool,
}

impl Ansi {
    /// 构造状态机。
    pub(crate) const fn new() -> Self {
        Self {
            state: 0,
            params: [0; 16],
            count: 0,
            value: 0,
            active: false,
        }
    }

    /// 送入一个字节。
    pub(crate) fn feed(&mut self, byte: u8) -> Event {
        match self.state {
            0 => {
                if byte == 0x1B {
                    self.state = 1;
                    Event::Ignore
                } else {
                    Event::Byte(byte)
                }
            }
            1 => match byte {
                b'[' => {
                    self.start();
                    Event::Ignore
                }
                b']' => {
                    self.state = 3;
                    Event::Ignore
                }
                _ => {
                    self.state = 0;
                    Event::Ignore
                }
            },
            2 => self.csi(byte),
            3 => {
                if byte == 0x07 {
                    self.state = 0;
                } else if byte == 0x1B {
                    self.state = 4;
                }
                Event::Ignore
            }
            4 => {
                self.state = if byte == b'\\' { 0 } else { 3 };
                Event::Ignore
            }
            _ => {
                self.state = 0;
                Event::Ignore
            }
        }
    }

    fn start(&mut self) {
        self.state = 2;
        self.count = 0;
        self.value = 0;
        self.active = false;
    }

    fn csi(&mut self, byte: u8) -> Event {
        if byte.is_ascii_digit() {
            self.value = self
                .value
                .saturating_mul(10)
                .saturating_add(u16::from(byte - b'0'));
            self.active = true;
            return Event::Ignore;
        }
        if byte == b';' {
            self.push();
            return Event::Ignore;
        }
        if byte == b'?' {
            return Event::Ignore;
        }
        if (0x40..=0x7E).contains(&byte) {
            if self.active || self.count > 0 {
                self.push();
            }
            self.state = 0;
            return match self.command(byte) {
                Some(command) => Event::Command(command),
                None => Event::Ignore,
            };
        }
        Event::Ignore
    }

    fn push(&mut self) {
        if self.count < self.params.len() {
            self.params[self.count] = self.value;
            self.count += 1;
        }
        self.value = 0;
        self.active = false;
    }

    fn param(&self, index: usize, default: u16) -> u16 {
        if index >= self.count {
            return default;
        }
        let value = self.params[index];
        if value == 0 { default } else { value }
    }

    fn command(&self, final_byte: u8) -> Option<Command> {
        Some(match final_byte {
            b'm' => Command::Sgr {
                params: self.params,
                count: self.count,
            },
            b'H' | b'f' => Command::CursorPosition {
                row: self.param(0, 1),
                column: self.param(1, 1),
            },
            b'A' => Command::CursorUp(self.param(0, 1)),
            b'B' => Command::CursorDown(self.param(0, 1)),
            b'C' => Command::CursorForward(self.param(0, 1)),
            b'D' => Command::CursorBack(self.param(0, 1)),
            b'G' => Command::CursorColumn(self.param(0, 1)),
            b'd' => Command::CursorRow(self.param(0, 1)),
            b'J' => Command::EraseDisplay(self.param(0, 0)),
            b'K' => Command::EraseLine(self.param(0, 0)),
            _ => return None,
        })
    }
}
