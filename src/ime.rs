//! librime 会话封装（计划 D3/D4）。

use std::path::Path;

use anyhow::{bail, Context as _, Result};
use rime_api::{
    create_session, full_deploy_and_wait, initialize, setup, DeployResult, Session, Traits,
};

/// 默认 rime 用户数据目录（首次初始化时 librime 自动部署到此）
pub fn default_user_data_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    format!("{home}/.local/share/tui-ime/rime")
}

/// 一个已初始化并部署完成的 rime 会话。
///
/// 注意：librime 全局状态每进程只能初始化一次（`initialize`），
/// 因此每进程只应创建一个 `Ime`。
pub struct Ime {
    session: Session,
}

/// 当前输入状态快照
pub struct Snapshot {
    pub preedit: String,
    pub candidates: Vec<String>,
    /// 当前页高亮候选序号
    pub highlighted: usize,
    pub page_no: usize,
    pub is_last_page: bool,
}

impl Ime {
    /// 初始化 librime（首次部署需 1-3 秒）并创建会话
    pub fn new(user_data_dir: &Path) -> Result<Self> {
        let mut traits = Traits::new();
        traits
            .set_shared_data_dir("/usr/share/rime-data")
            .set_user_data_dir(&user_data_dir.to_string_lossy())
            .set_distribution_name("tui-ime")
            .set_distribution_code_name("tui-ime")
            .set_distribution_version(env!("CARGO_PKG_VERSION"))
            .set_app_name("tui-ime")
            // 仅 FATAL 级别日志，避免 librime 日志污染终端
            .set_min_log_level(3);
        setup(&mut traits);
        initialize(&mut traits);
        match full_deploy_and_wait() {
            DeployResult::Success => {}
            DeployResult::Failure => bail!("rime deployment failed"),
        }
        let session = create_session().context("create rime session")?;
        Ok(Self { session })
    }

    /// 喂入一个按键；返回 true 表示被 rime 消费
    #[allow(dead_code)]
    pub fn process_key(&self, key_code: i32, modifiers: i32) -> bool {
        let status = self.session.process_key(rime_api::KeyEvent {
            key_code,
            modifiers,
        });
        status == rime_api::KeyStatus::Accept
    }

    /// 取出待上屏文本（应在每次 process_key 后调用）
    pub fn take_commit(&self) -> Option<String> {
        self.session.commit().map(|c| c.text().to_string())
    }

    /// 当前 preedit + 候选快照
    pub fn snapshot(&self) -> Snapshot {
        match self.session.context() {
            Some(ctx) => {
                let composition = ctx.composition();
                let menu = ctx.menu();
                Snapshot {
                    preedit: composition.preedit.unwrap_or_default().to_string(),
                    candidates: menu.candidates.iter().map(|c| c.text.to_string()).collect(),
                    highlighted: menu.highlighted_candidate_index,
                    page_no: menu.page_no,
                    is_last_page: menu.is_last_page,
                }
            }
            None => Snapshot {
                preedit: String::new(),
                candidates: Vec::new(),
                highlighted: 0,
                page_no: 0,
                is_last_page: true,
            },
        }
    }
}

impl Drop for Ime {
    fn drop(&mut self) {
        let _ = self.session.close();
    }
}

/// 全局收尾：销毁 librime 全局状态。
/// 必须在所有会话关闭后、进程退出前调用——否则 librime 的静态
/// `Service` 析构器会在 exit() 时对残留会话执行引擎析构并崩溃。
pub fn finalize() {
    rime_api::finalize();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// rime 端到端：init → 输入 "nihao" → 候选非空 → Space → commit 非空。
    /// 依赖系统 rime 数据且全局只能初始化一次，默认跳过；
    /// 手动运行：cargo test -- --ignored
    #[test]
    #[ignore]
    fn rime_end_to_end() {
        let dir = std::env::temp_dir().join(format!("tui-ime-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ime = Ime::new(&dir).unwrap();

        for &b in b"nihao" {
            ime.process_key(b as i32, 0);
        }
        let snap = ime.snapshot();
        // luna_pinyin 的 preedit 带分词空格（如 "ni hao"）
        assert_eq!(snap.preedit.replace(' ', ""), "nihao");
        assert!(
            !snap.candidates.is_empty(),
            "expected candidates for preedit 'nihao'"
        );

        ime.process_key(32, 0); // Space → 首选上屏
        let commit = ime.take_commit();
        assert!(
            commit.as_deref().is_some_and(|s| !s.is_empty()),
            "expected non-empty commit, got {commit:?}"
        );
    }
}
