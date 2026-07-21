//! 按键事件 → rime keycode/mask 映射。
//!
//! rime keycode 采用 X11 keysym（见 `rime_api.h`）。数值与 librime-sys
//! 生成的 `RimeKeyCode_XK_*` / `RimeModifier_*` 常量一致，此处内联常用值，
//! 避免对 librime-sys 的直接依赖（计划 C4 白名单仅含 rime-api）。

use crate::keyevent::{KeyEvent, ALT, CTRL, SHIFT};

// RimeModifier
pub const SHIFT_MASK: i32 = 1; // kShiftMask
pub const CTRL_MASK: i32 = 4; // kControlMask
pub const ALT_MASK: i32 = 8; // kAltMask

// RimeKeyCode（X11 keysym）
pub const XK_BACKSPACE: i32 = 65288;
pub const XK_TAB: i32 = 65289;
pub const XK_RETURN: i32 = 65293;
pub const XK_ESCAPE: i32 = 65307;
pub const XK_DELETE: i32 = 65535;
pub const XK_LEFT: i32 = 65361;
pub const XK_UP: i32 = 65362;
pub const XK_RIGHT: i32 = 65363;
pub const XK_DOWN: i32 = 65364;
pub const XK_PAGE_UP: i32 = 65365;
pub const XK_PAGE_DOWN: i32 = 65366;

/// legacy 字节 → (keycode, mask)；不可映射（C0 控制码等）返回 None
pub fn byte_to_rime(b: u8) -> Option<(i32, i32)> {
    match b {
        0x20..=0x7e => Some((b as i32, 0)),
        0x7f => Some((XK_BACKSPACE, 0)),
        b'\r' | b'\n' => Some((XK_RETURN, 0)),
        b'\t' => Some((XK_TAB, 0)),
        0x1b => Some((XK_ESCAPE, 0)),
        _ => None,
    }
}

/// kitty CSI 按键 → (keycode, mask)；不可映射返回 None
pub fn key_to_rime(k: &KeyEvent) -> Option<(i32, i32)> {
    let mut mask = 0;
    if k.modifiers & SHIFT != 0 {
        mask |= SHIFT_MASK;
    }
    if k.modifiers & CTRL != 0 {
        mask |= CTRL_MASK;
    }
    if k.modifiers & ALT != 0 {
        mask |= ALT_MASK;
    }
    match k.terminator {
        b'u' => match k.codepoint {
            9 => Some((XK_TAB, mask)),
            13 => Some((XK_RETURN, mask)),
            27 => Some((XK_ESCAPE, mask)),
            127 => Some((XK_BACKSPACE, mask)),
            0x20..=0x7e => Some((k.codepoint as i32, mask)),
            _ => None,
        },
        b'A' => Some((XK_UP, mask)),
        b'B' => Some((XK_DOWN, mask)),
        b'C' => Some((XK_RIGHT, mask)),
        b'D' => Some((XK_LEFT, mask)),
        b'~' => match k.codepoint {
            3 => Some((XK_DELETE, mask)),
            5 => Some((XK_PAGE_UP, mask)),
            6 => Some((XK_PAGE_DOWN, mask)),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyevent::EventType;

    #[test]
    fn byte_mapping() {
        assert_eq!(byte_to_rime(b'a'), Some((97, 0)));
        assert_eq!(byte_to_rime(b'1'), Some((49, 0)));
        assert_eq!(byte_to_rime(b' '), Some((32, 0)));
        assert_eq!(byte_to_rime(0x7f), Some((XK_BACKSPACE, 0)));
        assert_eq!(byte_to_rime(b'\r'), Some((XK_RETURN, 0)));
        assert_eq!(byte_to_rime(b'\t'), Some((XK_TAB, 0)));
        assert_eq!(byte_to_rime(0x1b), Some((XK_ESCAPE, 0)));
        assert_eq!(byte_to_rime(0x03), None); // Ctrl+C 控制字节不映射
    }

    fn key(codepoint: u32, modifiers: u8, terminator: u8) -> KeyEvent {
        KeyEvent {
            codepoint,
            modifiers,
            event_type: EventType::Press,
            terminator,
            raw: Vec::new(),
        }
    }

    #[test]
    fn key_mapping_printable() {
        assert_eq!(key_to_rime(&key(97, 0, b'u')), Some((97, 0)));
        assert_eq!(key_to_rime(&key(97, CTRL, b'u')), Some((97, CTRL_MASK)));
        assert_eq!(
            key_to_rime(&key(65, SHIFT | CTRL, b'u')),
            Some((65, SHIFT_MASK | CTRL_MASK))
        );
    }

    #[test]
    fn key_mapping_special() {
        assert_eq!(key_to_rime(&key(13, 0, b'u')), Some((XK_RETURN, 0)));
        assert_eq!(key_to_rime(&key(27, 0, b'u')), Some((XK_ESCAPE, 0)));
        assert_eq!(key_to_rime(&key(127, 0, b'u')), Some((XK_BACKSPACE, 0)));
        assert_eq!(key_to_rime(&key(9, 0, b'u')), Some((XK_TAB, 0)));
    }

    #[test]
    fn key_mapping_functional() {
        assert_eq!(key_to_rime(&key(1, 0, b'A')), Some((XK_UP, 0)));
        assert_eq!(key_to_rime(&key(1, 0, b'B')), Some((XK_DOWN, 0)));
        assert_eq!(key_to_rime(&key(1, 0, b'C')), Some((XK_RIGHT, 0)));
        assert_eq!(key_to_rime(&key(1, 0, b'D')), Some((XK_LEFT, 0)));
        assert_eq!(key_to_rime(&key(3, 0, b'~')), Some((XK_DELETE, 0)));
        assert_eq!(key_to_rime(&key(5, 0, b'~')), Some((XK_PAGE_UP, 0)));
        assert_eq!(key_to_rime(&key(6, 0, b'~')), Some((XK_PAGE_DOWN, 0)));
        assert_eq!(key_to_rime(&key(1, CTRL, b'A')), Some((XK_UP, CTRL_MASK)));
    }
}
