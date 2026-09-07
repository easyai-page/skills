//! UI 状态（$SKILLS_HOME/state.json）：交互向导的记忆数据（当前仅 agent 选择记忆）。
//! 与 registry 无关；缺失/损坏一律降级为默认空状态，绝不影响核心功能。

use serde::{Deserialize, Serialize};

use super::paths::Layout;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UiState {
    #[serde(default = "one")]
    pub version: u32,
    #[serde(default)]
    pub last_selected_agents: Vec<String>,
}

fn one() -> u32 {
    1
}

/// 读取 UI 状态；文件缺失或损坏都返回默认（记忆只是体验优化，不是权威数据）。
pub fn load(layout: &Layout) -> UiState {
    std::fs::read_to_string(layout.state_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// 记录上次交互确认的 agent 选择。写失败仅警告——记忆丢失不该阻塞安装。
pub fn save_last_agents(layout: &Layout, agents: &[String]) {
    let state = UiState {
        version: 1,
        last_selected_agents: agents.to_vec(),
    };
    if let Err(e) = write_atomic(layout, &state) {
        eprintln!("警告：UI 状态写入失败（{e}），agent 选择记忆未保存");
    }
}

/// tmp + rename 原子写（与 registry.save 同款语义，防半截文件）。
fn write_atomic(layout: &Layout, state: &UiState) -> Result<(), String> {
    let path = layout.state_path();
    let text = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::paths::Layout;

    #[test]
    fn missing_file_returns_default() {
        let tmp = tempfile::tempdir().unwrap();
        let l = Layout::at(tmp.path().to_path_buf());
        assert!(load(&l).last_selected_agents.is_empty());
    }

    #[test]
    fn save_then_load_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let l = Layout::at(tmp.path().to_path_buf());
        save_last_agents(&l, &["claude-code".to_string(), "codex".to_string()]);
        assert_eq!(load(&l).last_selected_agents, vec!["claude-code", "codex"]);
    }

    #[test]
    fn corrupted_file_returns_default() {
        let tmp = tempfile::tempdir().unwrap();
        let l = Layout::at(tmp.path().to_path_buf());
        std::fs::write(l.state_path(), "not json {{{").unwrap();
        assert!(load(&l).last_selected_agents.is_empty());
    }
}
