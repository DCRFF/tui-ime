//! IME 组合 UI 的 inline ANSI 渲染（Phase 2 计划 D1/D2/D3）。
//!
//! preedit + 单行候选条绘制在光标行：`\e7` 保存光标 → 样式文本 →
//! 尾部空格擦除 → `\e8` 恢复。渲染不变式：绘制结束后光标仍在条带起点，
//! 下次 draw/erase 从同一位置开始（组合期间 shell 无输出，光标不动）。
//!
//! 光标行横向空间不足（行尾盲打场景）时，改用 IL/DL（`\e[L`/`\e[M` +
//! DECSTBM 滚动区域）在光标下一行"借"一个空白行绘制，擦除时对称删除、
//! 内容原样移回（zsh-autocomplete 同款机制）。光标在屏幕最后一行时
//! 无法借行，回退 inline 截断。

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

/// 条带位置：inline（光标处）或 below（借光标下一行，行尾空间不足时）
#[derive(PartialEq, Clone, Copy, Debug)]
enum Place {
    Inline,
    Below,
}

/// inline 可用宽度低于该值时，条带搬到借来的下一行（行尾盲打兜底）
const MIN_INLINE_WIDTH: usize = 16;

/// 渲染器：记住上次绘制宽度以精确擦除；知道光标行列以精确截断/借行
pub struct Renderer {
    prev_width: usize,
    /// 组合开始时光标列（1-based，DSR 查询结果）；None 时用回退上限
    cursor_col: Option<u16>,
    /// 组合开始时光标行（1-based，DSR 查询结果）；借行决策需要
    cursor_row: Option<u16>,
    /// 最近一次 draw 的终端行数（借行/还行的滚动区域上限）
    term_rows: u16,
    place: Place,
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
            cursor_row: None,
            term_rows: 24,
            place: Place::Inline,
        }
    }

    pub fn set_cursor_pos(&mut self, row: u16, col: u16) {
        self.cursor_row = Some(row);
        self.cursor_col = Some(col);
    }

    /// 组合结束/关闭时复位列信息（下次组合重新查询）
    pub fn reset(&mut self) {
        self.cursor_col = None;
        self.cursor_row = None;
        self.place = Place::Inline;
    }

    /// 当前是否有已绘制内容（测试用）
    #[allow(dead_code)]
    pub fn is_drawn(&self) -> bool {
        self.prev_width > 0
    }

    /// 组合快照 → 完整绘制字节串
    pub fn draw(&mut self, strip: &Strip, term_cols: u16, term_rows: u16) -> Vec<u8> {
        self.term_rows = term_rows;
        // 借行条件：已知光标行列 + inline 宽度不足 + 光标不在最后一行
        // （光标下方为空行时 IL/DL 对称可逆；下方有内容的全屏程序为已知边界）
        let borrow = self.cursor_col.is_some()
            && self.avail_width(term_cols) < MIN_INLINE_WIDTH
            && matches!(self.cursor_row, Some(r) if r < term_rows);

        let mut out = Vec::new();
        if borrow && self.place == Place::Inline {
            out.extend(self.erase_inline());
            out.extend(self.borrow_line());
            self.place = Place::Below;
        } else if !borrow && self.place == Place::Below {
            out.extend(self.return_line());
            self.place = Place::Inline;
        }
        match self.place {
            Place::Inline => out.extend(self.draw_inline(strip, term_cols)),
            Place::Below => out.extend(self.draw_below(strip, term_cols)),
        }
        out
    }

    /// 擦除上次绘制（inline 盖空格；below 删除借来的行）
    pub fn erase(&mut self) -> Vec<u8> {
        match self.place {
            Place::Inline => self.erase_inline(),
            Place::Below => {
                self.place = Place::Inline;
                self.prev_width = 0;
                self.return_line()
            }
        }
    }

    /// inline 绘制：光标处 `\e7` 保存 → 样式文本 → 尾部空格擦除 → `\e8` 恢复
    fn draw_inline(&mut self, strip: &Strip, term_cols: u16) -> Vec<u8> {
        let avail = self.avail_width(term_cols);
        let tokens = build_tokens(strip, avail);
        let used: usize = tokens.iter().map(|t| width_of(&t.text)).sum();

        let mut out = Vec::new();
        out.extend_from_slice(b"\x1b[?25l"); // 隐藏光标
        out.extend_from_slice(b"\x1b7"); // 保存光标
        emit_tokens(&mut out, &tokens);
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

    /// below 绘制：在借来的行（光标下一行）从第 1 列整行绘制
    fn draw_below(&mut self, strip: &Strip, term_cols: u16) -> Vec<u8> {
        let row = self.cursor_row.expect("cursor row for below draw") + 1;
        let tokens = build_tokens(strip, term_cols as usize);
        let used: usize = tokens.iter().map(|t| width_of(&t.text)).sum();

        let mut out = Vec::new();
        out.extend_from_slice(b"\x1b[?25l\x1b7");
        out.extend_from_slice(format!("\x1b[{row};1H").as_bytes());
        emit_tokens(&mut out, &tokens);
        if used < self.prev_width {
            out.extend(std::iter::repeat_n(b' ', self.prev_width - used));
        }
        out.extend_from_slice(b"\x1b[0m\x1b8\x1b[?25h");
        self.prev_width = used;
        out
    }

    /// inline 擦除：上次绘制宽度盖空格
    fn erase_inline(&mut self) -> Vec<u8> {
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

    /// 借行：滚动区域设为光标行下方 → 光标下一行 IL 插入空白行 → 还原区域。
    /// 下方内容随插入下移一行；对称的 return_line 会将其移回。
    fn borrow_line(&self) -> Vec<u8> {
        let row = self.cursor_row.expect("cursor row for borrow") + 1;
        let rows = self.term_rows;
        let mut out = Vec::new();
        out.extend_from_slice(b"\x1b[?25l\x1b7");
        out.extend_from_slice(format!("\x1b[{row};{rows}r").as_bytes()); // 滚动区域
        out.extend_from_slice(format!("\x1b[{row};1H").as_bytes());
        out.extend_from_slice(b"\x1b[L"); // 插入空白行
        out.extend_from_slice(b"\x1b[r"); // 还原滚动区域（光标归 \e8 恢复）
        out.extend_from_slice(b"\x1b8\x1b[?25h");
        out
    }

    /// 还行：删除借来的行，下方内容移回原位
    fn return_line(&self) -> Vec<u8> {
        let row = self.cursor_row.expect("cursor row for return") + 1;
        let rows = self.term_rows;
        let mut out = Vec::new();
        out.extend_from_slice(b"\x1b[?25l\x1b7");
        out.extend_from_slice(format!("\x1b[{row};{rows}r").as_bytes());
        out.extend_from_slice(format!("\x1b[{row};1H").as_bytes());
        out.extend_from_slice(b"\x1b[M"); // 删除借来的行
        out.extend_from_slice(b"\x1b[r");
        out.extend_from_slice(b"\x1b8\x1b[?25h");
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

/// 样式 token 输出（inline/below 共用）
fn emit_tokens(out: &mut Vec<u8>, tokens: &[Token]) {
    out.extend_from_slice(b"\x1b[2m"); // dim 开始
    for t in tokens {
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
        r.set_cursor_pos(1, 3);
        let out = r.draw(&strip(), 80, 24);
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
        r.set_cursor_pos(1, 60); // 仅剩 21 列：preedit(6) + " 1.你好"(7) + " 2.您好"(7) = 20
        let out = r.draw(&strip(), 80, 24);
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains(" 2.您好"));
        assert!(!s.contains(" 3.尼好")); // 放不下的候选从尾部丢弃
    }

    #[test]
    fn fallback_width_without_dsr() {
        let mut r = Renderer::new(); // 无光标位置 → 60% * 80 = 48 列
        let out = r.draw(&strip(), 80, 24);
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains(" 3.尼好"));
    }

    #[test]
    fn page_indicator_only_when_multi_page() {
        let mut r = Renderer::new();
        r.set_cursor_pos(1, 1);
        let mut s1 = strip();
        let out = r.draw(&s1, 80, 24);
        assert!(!String::from_utf8_lossy(&out).contains('('));

        s1.page_no = 1;
        s1.is_last_page = false;
        let out = r.draw(&s1, 80, 24);
        assert!(String::from_utf8_lossy(&out).contains("(2+)"));
    }

    #[test]
    fn erase_covers_previous_width_with_spaces() {
        let mut r = Renderer::new();
        r.set_cursor_pos(1, 1);
        r.draw(&strip(), 80, 24);
        let w1 = r.prev_width;

        // 更短的 strip：尾部应补空格擦除
        let short = Strip {
            preedit: "ni".to_string(),
            ..Default::default()
        };
        let out = r.draw(&short, 80, 24);
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

    // --- 借行模式（行尾横向空间不足） ---

    #[test]
    fn borrow_line_at_right_edge() {
        let mut r = Renderer::new();
        r.set_cursor_pos(10, 78); // 80 列终端仅剩 3 列 < MIN_INLINE_WIDTH
        let out = r.draw(&strip(), 80, 24);
        let s = String::from_utf8_lossy(&out);
        // 借行：滚动区域 11;24 + IL 插入空白行
        assert!(s.contains("\x1b[11;24r"), "expected scroll region, got {s:?}");
        assert!(s.contains("\x1b[L"), "expected insert line");
        // 条带画在第 11 行，内容完整不截断
        assert!(s.contains("\x1b[11;1H"));
        assert!(s.contains(" 3.尼好"));
        assert!(r.is_drawn());
    }

    #[test]
    fn borrowed_erase_deletes_line() {
        let mut r = Renderer::new();
        r.set_cursor_pos(10, 78);
        r.draw(&strip(), 80, 24);
        let erased = r.erase();
        let s = String::from_utf8_lossy(&erased);
        assert!(s.contains("\x1b[M"), "expected delete line, got {s:?}");
        assert!(!s.contains("    "), "below 模式不应盖空格");
        assert!(!r.is_drawn());
        // 还行后回到 inline，幂等
        assert!(r.erase().is_empty());
    }

    #[test]
    fn no_borrow_on_last_row() {
        let mut r = Renderer::new();
        r.set_cursor_pos(24, 78); // 最后一行无法借行 → inline 截断兜底
        let out = r.draw(&strip(), 80, 24);
        let s = String::from_utf8_lossy(&out);
        assert!(!s.contains("\x1b[L"));
        assert!(s.contains("ni ")); // 只剩 3 列，preedit 被截断
    }

    #[test]
    fn inline_to_borrowed_transition_erases_inline_first() {
        let mut r = Renderer::new(); // 无 DSR：先按 60% inline 画
        r.draw(&strip(), 80, 24);
        assert!(r.is_drawn());
        let w1 = r.prev_width;

        // DSR 到达：发现横向不足 → 先擦 inline（盖空格）再借行
        r.set_cursor_pos(10, 78);
        let out = r.draw(&strip(), 80, 24);
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains(&" ".repeat(w1)), "expected inline erase spaces");
        assert!(s.contains("\x1b[L"));
    }
}
