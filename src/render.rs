//! IME 组合 UI 的 inline ANSI 渲染（Phase 2 计划 D1/D2/D3）。
//!
//! preedit + 单行候选条绘制在光标行：`\e7` 保存光标 → 样式文本 →
//! 尾部空格擦除 → `\e8` 恢复。渲染不变式：绘制结束后光标仍在条带起点，
//! 下次 draw/erase 从同一位置开始（组合期间 shell 无输出，光标不动）。

use unicode_width::UnicodeWidthChar;

/// 一次组合状态的快照（镜像自 rime context）
#[derive(Debug)]
pub struct Strip {
    pub preedit: String,
    /// 当前页候选文本（已按页裁剪）
    pub candidates: Vec<String>,
    /// 高亮候选在 candidates 中的序号
    pub highlighted: usize,
    pub page_no: usize,
    pub is_last_page: bool,
}

impl Default for Strip {
    fn default() -> Self {
        Self {
            preedit: String::new(),
            candidates: Vec::new(),
            highlighted: 0,
            page_no: 0,
            is_last_page: true,
        }
    }
}

impl Strip {
    pub fn is_active(&self) -> bool {
        !self.preedit.is_empty()
    }
}

/// 渲染器：记住上次绘制宽度以精确擦除；知道光标列以精确截断
pub struct Renderer {
    prev_width: usize,
    /// 组合开始时光标列（1-based，DSR 查询结果）；None 时用回退上限
    cursor_col: Option<u16>,
}

/// 样式 token：文本片段 + 是否高亮
struct Token {
    text: String,
    style: Style,
}

#[derive(PartialEq)]
enum Style {
    /// preedit：下划线 + dim
    Preedit,
    /// 普通候选：dim
    Plain,
    /// 高亮候选：dim + 反显
    Highlight,
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            prev_width: 0,
            cursor_col: None,
        }
    }

    pub fn set_cursor_col(&mut self, col: u16) {
        self.cursor_col = Some(col);
    }

    /// 组合结束/关闭时复位列信息（下次组合重新查询）
    pub fn reset(&mut self) {
        self.cursor_col = None;
    }

    /// 当前是否有已绘制内容（测试用）
    #[allow(dead_code)]
    pub fn is_drawn(&self) -> bool {
        self.prev_width > 0
    }

    /// 组合快照 → 完整绘制字节串
    pub fn draw(&mut self, strip: &Strip, term_cols: u16) -> Vec<u8> {
        let avail = self.avail_width(term_cols);
        let tokens = build_tokens(strip, avail);
        let used: usize = tokens.iter().map(|t| width_of(&t.text)).sum();

        let mut out = Vec::new();
        out.extend_from_slice(b"\x1b[?25l"); // 隐藏光标
        out.extend_from_slice(b"\x1b7"); // 保存光标
        out.extend_from_slice(b"\x1b[2m"); // dim 开始
        for t in &tokens {
            match t.style {
                Style::Preedit => {
                    out.extend_from_slice(b"\x1b[4m");
                    out.extend_from_slice(t.text.as_bytes());
                    out.extend_from_slice(b"\x1b[24m");
                }
                Style::Plain => out.extend_from_slice(t.text.as_bytes()),
                Style::Highlight => {
                    out.extend_from_slice(b"\x1b[7m");
                    out.extend_from_slice(t.text.as_bytes());
                    out.extend_from_slice(b"\x1b[27m");
                }
            }
        }
        // 尾部擦除：本次比上次短时，用空格盖住残余
        if used < self.prev_width {
            out.extend(std::iter::repeat_n(b' ', self.prev_width - used));
        }
        out.extend_from_slice(b"\x1b[0m"); // 复位样式
        out.extend_from_slice(b"\x1b8"); // 恢复光标
        out.extend_from_slice(b"\x1b[?25h"); // 显示光标
        self.prev_width = used;
        out
    }

    /// 擦除上次绘制
    pub fn erase(&mut self) -> Vec<u8> {
        if self.prev_width == 0 {
            return Vec::new();
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"\x1b[?25l");
        out.extend_from_slice(b"\x1b7");
        out.extend(std::iter::repeat_n(b' ', self.prev_width));
        out.extend_from_slice(b"\x1b8");
        out.extend_from_slice(b"\x1b[?25h");
        self.prev_width = 0;
        out
    }

    /// 可用宽度：光标列起至行尾；无列信息时回退 60% 列宽（计划 D3）
    fn avail_width(&self, term_cols: u16) -> usize {
        match self.cursor_col {
            Some(col) => term_cols.saturating_sub(col).saturating_add(1) as usize,
            None => (term_cols as usize) * 60 / 100,
        }
    }
}

/// 组装修剪后的 token 序列：preedit 永远保留，候选从尾部丢弃（L4）
fn build_tokens(strip: &Strip, avail: usize) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut used = 0usize;

    let preedit = truncate_str(&strip.preedit, avail);
    used += width_of(&preedit);
    tokens.push(Token {
        text: preedit,
        style: Style::Preedit,
    });

    for (i, cand) in strip.candidates.iter().enumerate() {
        let token_text = format!(" {}.{}", i + 1, cand);
        let w = width_of(&token_text);
        if used + w > avail {
            break;
        }
        used += w;
        tokens.push(Token {
            text: token_text,
            style: if i == strip.highlighted {
                Style::Highlight
            } else {
                Style::Plain
            },
        });
    }

    // 多页时显示页码，如 "(2+)"（第 2 页，还有下一页）
    if strip.page_no > 0 || !strip.is_last_page {
        let page = format!(
            " ({}{})",
            strip.page_no + 1,
            if strip.is_last_page { "" } else { "+" }
        );
        let w = width_of(&page);
        if used + w <= avail {
            tokens.push(Token {
                text: page,
                style: Style::Plain,
            });
        }
    }

    tokens
}

fn width_of(s: &str) -> usize {
    s.chars()
        .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
        .sum()
}

/// 按列宽截断（不按字节），CJK 字符宽 2
fn truncate_str(s: &str, max: usize) -> String {
    let mut used = 0;
    s.chars()
        .take_while(|c| {
            let w = UnicodeWidthChar::width(*c).unwrap_or(0);
            if used + w <= max {
                used += w;
                true
            } else {
                false
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip() -> Strip {
        Strip {
            preedit: "ni hao".to_string(),
            candidates: vec!["你好".to_string(), "您好".to_string(), "尼好".to_string()],
            highlighted: 0,
            page_no: 0,
            is_last_page: true,
        }
    }

    #[test]
    fn cjk_width() {
        assert_eq!(width_of("你好"), 4);
        assert_eq!(width_of("ni hao"), 6);
        assert_eq!(width_of(" 1.你好"), 7);
    }

    #[test]
    fn draw_contains_styled_strip_and_restores_cursor() {
        let mut r = Renderer::new();
        r.set_cursor_col(3);
        let out = r.draw(&strip(), 80);
        let s = String::from_utf8_lossy(&out);
        assert!(s.starts_with("\x1b[?25l\x1b7\x1b[2m"));
        assert!(s.contains("\x1b[4mni hao\x1b[24m")); // preedit 下划线
        assert!(s.contains("\x1b[7m 1.你好\x1b[27m")); // 高亮反显
        assert!(s.contains(" 2.您好"));
        assert!(s.ends_with("\x1b[0m\x1b8\x1b[?25h"));
        assert!(r.is_drawn());
    }

    #[test]
    fn truncate_drops_tail_candidates() {
        let mut r = Renderer::new();
        r.set_cursor_col(60); // 仅剩 21 列：preedit(6) + " 1.你好"(7) + " 2.您好"(7) = 20
        let out = r.draw(&strip(), 80);
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains(" 2.您好"));
        assert!(!s.contains(" 3.尼好")); // 放不下的候选从尾部丢弃
    }

    #[test]
    fn fallback_width_without_dsr() {
        let mut r = Renderer::new(); // 无 cursor_col → 60% * 80 = 48 列
        let out = r.draw(&strip(), 80);
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains(" 3.尼好"));
    }

    #[test]
    fn page_indicator_only_when_multi_page() {
        let mut r = Renderer::new();
        r.set_cursor_col(1);
        let mut s1 = strip();
        let out = r.draw(&s1, 80);
        assert!(!String::from_utf8_lossy(&out).contains('('));

        s1.page_no = 1;
        s1.is_last_page = false;
        let out = r.draw(&s1, 80);
        assert!(String::from_utf8_lossy(&out).contains("(2+)"));
    }

    #[test]
    fn erase_covers_previous_width_with_spaces() {
        let mut r = Renderer::new();
        r.set_cursor_col(1);
        r.draw(&strip(), 80);
        let w1 = r.prev_width;

        // 更短的 strip：尾部应补空格擦除
        let short = Strip {
            preedit: "ni".to_string(),
            ..Default::default()
        };
        let out = r.draw(&short, 80);
        let s = String::from_utf8_lossy(&out);
        // 短内容 + (w1 - 2) 个空格
        assert!(s.contains(&" ".repeat(w1 - 2)));

        let erased = r.erase();
        assert_eq!(
            erased,
            [
                b"\x1b[?25l\x1b7".as_slice(),
                &" ".repeat(2).into_bytes(),
                b"\x1b8\x1b[?25h"
            ]
            .concat()
        );
        assert!(!r.is_drawn());
        assert!(r.erase().is_empty()); // 幂等
    }
}
