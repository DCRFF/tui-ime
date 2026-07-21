//! Kitty keyboard protocol（CSI u）增量解析与 legacy 回译。
//!
//! 终端在 flags=0b1（disambiguate，计划 D8）下：普通可打印键仍走 legacy
//! 字节，仅修饰组合键 / 特殊键以 CSI 序列上报。本模块职责：
//! - 增量解析 CSI 序列 → [`KeyEvent`]（跨 read 边界缓冲）
//! - IME 关闭时把无 legacy 等价物的序列回译为 legacy 字节（如 ctrl+a → 0x01），
//!   使不了解 kitty protocol 的 shell/readline 正常工作

/// 修饰键 bitmask（kitty 上报值减 1）
pub const SHIFT: u8 = 1;
pub const ALT: u8 = 2;
pub const CTRL: u8 = 4;
#[allow(dead_code)]
pub const SUPER: u8 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventType {
    Press,
    Repeat,
    Release,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    /// Unicode 码点；功能键（方向键等）为 0
    pub codepoint: u32,
    /// 修饰键 bitmask（SHIFT/ALT/CTRL/SUPER 组合）
    pub modifiers: u8,
    pub event_type: EventType,
    /// 终结字节：b'u' / b'~' / b'A' 等功能键终结符
    pub terminator: u8,
    /// 原始字节，用于无需处理时原样转发
    pub raw: Vec<u8>,
}

impl KeyEvent {
    pub fn is_press(&self) -> bool {
        self.event_type == EventType::Press
    }

    /// Ctrl+Space —— IME 开关
    pub fn is_toggle(&self) -> bool {
        self.terminator == b'u' && self.codepoint == 32 && self.modifiers == CTRL
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    /// 普通 legacy 字节
    Byte(u8),
    /// 完整解析的 CSI 按键
    Key(KeyEvent),
    /// DSR（`\e[6n`）响应 `\e[<row>;<col>R`；非查询 pending 时应原样转发 raw
    Dsr { row: u16, col: u16, raw: Vec<u8> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Esc,
    Csi,
}

/// 增量 CSI 解析器：任意分片的字节流喂入，产出事件序列
pub struct Parser {
    state: State,
    buf: Vec<u8>,
}

impl Parser {
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            buf: Vec::new(),
        }
    }

    pub fn feed(&mut self, input: &[u8]) -> Vec<InputEvent> {
        let mut events = Vec::new();
        for &b in input {
            match self.state {
                State::Ground => {
                    if b == 0x1b {
                        self.state = State::Esc;
                        self.buf.push(b);
                    } else {
                        events.push(InputEvent::Byte(b));
                    }
                }
                State::Esc => {
                    if b == b'[' {
                        self.state = State::Csi;
                        self.buf.push(b);
                    } else if b == 0x1b {
                        // 连续 ESC：前一个按普通字节处理
                        events.push(InputEvent::Byte(0x1b));
                    } else {
                        // \eX（如 legacy Alt 前缀）：原样吐出
                        events.push(InputEvent::Byte(0x1b));
                        events.push(InputEvent::Byte(b));
                        self.buf.clear();
                        self.state = State::Ground;
                    }
                }
                State::Csi => {
                    self.buf.push(b);
                    if b.is_ascii_digit() || b == b';' || b == b':' {
                        // 参数字节，继续收集
                    } else if (0x40..=0x7e).contains(&b) {
                        if b == b'R' {
                            // DSR 响应（或 F3 修饰键冲突，由上层按 pending 状态裁决）
                            match parse_dsr(&self.buf) {
                                Some((row, col)) => events.push(InputEvent::Dsr {
                                    row,
                                    col,
                                    raw: self.buf.clone(),
                                }),
                                None => events.extend(self.buf.drain(..).map(InputEvent::Byte)),
                            }
                        } else if let Some(ev) = parse_csi(&self.buf) {
                            events.push(InputEvent::Key(ev));
                        } else {
                            events.extend(self.buf.drain(..).map(InputEvent::Byte));
                        }
                        self.buf.clear();
                        self.state = State::Ground;
                    } else {
                        // 非法参数字节：原样吐出
                        events.extend(self.buf.drain(..).map(InputEvent::Byte));
                        self.state = State::Ground;
                    }
                }
            }
        }
        events
    }

    /// 冲刷未完成序列（按 raw bytes 处理）
    #[allow(dead_code)]
    pub fn flush(&mut self) -> Vec<InputEvent> {
        self.state = State::Ground;
        self.buf.drain(..).map(InputEvent::Byte).collect()
    }
}

/// 解析完整 CSI 序列（`\e[` + 参数 + 终结字节）
fn parse_csi(raw: &[u8]) -> Option<KeyEvent> {
    if raw.len() < 3 {
        return None;
    }
    let terminator = *raw.last().unwrap();
    let params = &raw[2..raw.len() - 1];
    let mut fields = params.split(|&b| b == b';');
    let f1 = fields.next().unwrap_or(&[]);
    let f2 = fields.next();
    if fields.next().is_some() {
        return None; // 不支持更多段（如 text-as-codepoints）
    }

    let codepoint = parse_num(first_subfield(f1))?;
    let (modifiers, event_type) = match f2 {
        None => (0, EventType::Press),
        Some(f) => {
            let mut subs = f.split(|&b| b == b':');
            let m = parse_num(subs.next().unwrap())?;
            // kitty 修饰键值 = bitmask + 1，缺省/0 视为 1（无修饰）
            let mods = (if m == 0 { 1 } else { m }).saturating_sub(1) as u8;
            let ev = match subs.next() {
                None => EventType::Press,
                Some(s) => match parse_num(s)? {
                    2 => EventType::Repeat,
                    3 => EventType::Release,
                    _ => EventType::Press,
                },
            };
            (mods, ev)
        }
    };

    Some(KeyEvent {
        codepoint,
        modifiers,
        event_type,
        terminator,
        raw: raw.to_vec(),
    })
}

fn first_subfield(f: &[u8]) -> &[u8] {
    f.split(|&b| b == b':').next().unwrap()
}

/// 解析 DSR 响应 `\e[<row>;<col>R`：恰好两个纯数字字段
fn parse_dsr(raw: &[u8]) -> Option<(u16, u16)> {
    if raw.len() < 6 {
        return None;
    }
    let params = &raw[2..raw.len() - 1];
    let mut fields = params.split(|&b| b == b';');
    let row = parse_num(fields.next()?)?;
    let col = parse_num(fields.next()?)?;
    if fields.next().is_some() {
        return None;
    }
    Some((row as u16, col as u16))
}

/// 解析十进制数；空字段视为 0；含非数字返回 None
fn parse_num(f: &[u8]) -> Option<u32> {
    let mut v: u32 = 0;
    for &b in f {
        if !b.is_ascii_digit() {
            return None;
        }
        v = v.checked_mul(10)?.checked_add((b - b'0') as u32)?;
    }
    Some(v)
}

/// IME 关闭时的 kitty→legacy 回译。
/// 返回 `Some(bytes)`：回译结果；返回 `None`：序列本身已 legacy 兼容，原样转发 raw。
pub fn to_legacy(ev: &KeyEvent) -> Option<Vec<u8>> {
    if ev.terminator != b'u' {
        return None; // 功能键序列（方向键/PgUp/F1 等）已 legacy 兼容
    }
    let cp = ev.codepoint;
    match ev.modifiers {
        0 => Some(encode_codepoint(cp)),
        SHIFT => match cp {
            9 => Some(b"\x1b[Z".to_vec()),   // Shift+Tab → backtab
            13 => Some(b"\r".to_vec()),      // Shift+Enter → Enter
            _ => Some(encode_codepoint(cp)), // 可打印键：Shift 已由终端体现在码点上
        },
        ALT => Some(with_esc(encode_codepoint(cp))),
        m if m == CTRL || m == CTRL | SHIFT => ctrl_byte(cp).map(|b| vec![b]),
        m if m == ALT | SHIFT => Some(with_esc(encode_codepoint(cp))),
        m if m == ALT | CTRL => ctrl_byte(cp).map(|b| with_esc(vec![b])),
        _ => None, // Super 等：原样转发（best effort）
    }
}

/// Ctrl+key → C0 控制字节（ctrl+a → 0x01，ctrl+space → 0x00，ctrl+[ → 0x1b ...）
fn ctrl_byte(cp: u32) -> Option<u8> {
    (0x20..=0x7f).contains(&cp).then_some((cp as u8) & 0x1f)
}

fn encode_codepoint(cp: u32) -> Vec<u8> {
    match char::from_u32(cp) {
        Some(c) => {
            let mut buf = [0u8; 4];
            c.encode_utf8(&mut buf).as_bytes().to_vec()
        }
        None => Vec::new(),
    }
}

fn with_esc(mut v: Vec<u8>) -> Vec<u8> {
    v.insert(0, 0x1b);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(raw: &[u8]) -> KeyEvent {
        let mut p = Parser::new();
        let events = p.feed(raw);
        match events.as_slice() {
            [InputEvent::Key(ev)] => ev.clone(),
            other => panic!("expected single Key event, got {other:?}"),
        }
    }

    #[test]
    fn parse_plain_key() {
        let ev = key(b"\x1b[97u");
        assert_eq!(ev.codepoint, 97);
        assert_eq!(ev.modifiers, 0);
        assert_eq!(ev.event_type, EventType::Press);
        assert_eq!(ev.terminator, b'u');
        assert_eq!(ev.raw, b"\x1b[97u");
    }

    #[test]
    fn parse_ctrl_space() {
        let ev = key(b"\x1b[32;5u");
        assert_eq!(ev.codepoint, 32);
        assert_eq!(ev.modifiers, CTRL);
        assert!(ev.is_toggle());
    }

    #[test]
    fn parse_release_event() {
        let ev = key(b"\x1b[97;5:3u");
        assert_eq!(ev.codepoint, 97);
        assert_eq!(ev.modifiers, CTRL);
        assert_eq!(ev.event_type, EventType::Release);
        assert!(!ev.is_press());
    }

    #[test]
    fn parse_dsr_response() {
        let mut p = Parser::new();
        let events = p.feed(b"\x1b[7;42R");
        match events.as_slice() {
            [InputEvent::Dsr { row, col, raw }] => {
                assert_eq!(*row, 7);
                assert_eq!(*col, 42);
                assert_eq!(raw, b"\x1b[7;42R");
            }
            other => panic!("expected Dsr event, got {other:?}"),
        }
    }

    #[test]
    fn dsr_fragmented() {
        let mut p = Parser::new();
        assert!(p.feed(b"\x1b[7;").is_empty());
        match p.feed(b"42R").as_slice() {
            [InputEvent::Dsr {
                row: 7, col: 42, ..
            }] => {}
            other => panic!("expected Dsr event, got {other:?}"),
        }
    }

    #[test]
    fn parse_functional_keys() {
        let ev = key(b"\x1b[1;5A"); // Ctrl+Right
        assert_eq!(ev.codepoint, 1);
        assert_eq!(ev.modifiers, CTRL);
        assert_eq!(ev.terminator, b'A');

        let ev = key(b"\x1b[3~"); // Delete
        assert_eq!(ev.codepoint, 3);
        assert_eq!(ev.modifiers, 0);
        assert_eq!(ev.terminator, b'~');
    }

    #[test]
    fn parse_fragmented() {
        let mut p = Parser::new();
        assert!(p.feed(b"\x1b[3").is_empty());
        assert!(p.feed(b"2;").is_empty());
        let events = p.feed(b"5u");
        match events.as_slice() {
            [InputEvent::Key(ev)] => assert!(ev.is_toggle()),
            other => panic!("expected toggle Key, got {other:?}"),
        }
    }

    #[test]
    fn passthrough_plain_bytes() {
        let mut p = Parser::new();
        let events = p.feed(b"abc");
        assert_eq!(
            events,
            vec![
                InputEvent::Byte(b'a'),
                InputEvent::Byte(b'b'),
                InputEvent::Byte(b'c')
            ]
        );
    }

    #[test]
    fn mixed_stream() {
        let mut p = Parser::new();
        let events = p.feed(b"a\x1b[32;5ub");
        assert_eq!(events.len(), 3);
        assert_eq!(events[0], InputEvent::Byte(b'a'));
        assert!(matches!(&events[1], InputEvent::Key(k) if k.is_toggle()));
        assert_eq!(events[2], InputEvent::Byte(b'b'));
    }

    #[test]
    fn legacy_alt_prefix_not_swallowed() {
        let mut p = Parser::new();
        let events = p.feed(b"\x1bx");
        assert_eq!(events, vec![InputEvent::Byte(0x1b), InputEvent::Byte(b'x')]);
    }

    #[test]
    fn incomplete_sequence_flushes_raw() {
        let mut p = Parser::new();
        assert!(p.feed(b"\x1b[32").is_empty());
        let events = p.flush();
        assert_eq!(
            events,
            vec![
                InputEvent::Byte(0x1b),
                InputEvent::Byte(b'['),
                InputEvent::Byte(b'3'),
                InputEvent::Byte(b'2')
            ]
        );
    }

    // --- legacy 回译 ---

    #[test]
    fn legacy_plain_key() {
        let ev = key(b"\x1b[97u");
        assert_eq!(to_legacy(&ev), Some(b"a".to_vec()));
    }

    #[test]
    fn legacy_control_keys() {
        assert_eq!(to_legacy(&key(b"\x1b[27u")), Some(vec![0x1b])); // Esc
        assert_eq!(to_legacy(&key(b"\x1b[13u")), Some(b"\r".to_vec())); // Enter
        assert_eq!(to_legacy(&key(b"\x1b[9u")), Some(b"\t".to_vec())); // Tab
        assert_eq!(to_legacy(&key(b"\x1b[127u")), Some(vec![0x7f])); // Backspace
    }

    #[test]
    fn legacy_ctrl_combos() {
        assert_eq!(to_legacy(&key(b"\x1b[97;5u")), Some(vec![0x01])); // Ctrl+a
        assert_eq!(to_legacy(&key(b"\x1b[32;5u")), Some(vec![0x00])); // Ctrl+Space
        assert_eq!(to_legacy(&key(b"\x1b[99;6u")), Some(vec![0x03])); // Ctrl+Shift+c
    }

    #[test]
    fn legacy_alt_combos() {
        assert_eq!(to_legacy(&key(b"\x1b[120;3u")), Some(vec![0x1b, b'x'])); // Alt+x
        assert_eq!(to_legacy(&key(b"\x1b[98;7u")), Some(vec![0x1b, 0x02])); // Alt+Ctrl+b
    }

    #[test]
    fn legacy_shift_tab() {
        assert_eq!(to_legacy(&key(b"\x1b[9;2u")), Some(b"\x1b[Z".to_vec()));
    }

    #[test]
    fn legacy_compatible_sequences_pass_raw() {
        let ev = key(b"\x1b[1;5A"); // Ctrl+Right：xterm legacy 同款编码
        assert_eq!(to_legacy(&ev), None);
        let ev = key(b"\x1b[3~"); // Delete
        assert_eq!(to_legacy(&ev), None);
    }
}
