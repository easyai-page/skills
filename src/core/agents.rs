//! agent 注册表：内置表移植自 vercel-labs/skills 的 src/agents.ts（数据同步，非代码依赖）。
//! 「装到哪」的唯一权威来源；config.toml [targets] 只做全局路径覆盖与自定义扩展。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::error::{Error, Result};

#[derive(Clone, Debug)]
pub struct AgentSpec {
    pub id: &'static str,
    pub name: &'static str,
    /// 项目内相对路径（项目级安装 = <项目根>/<project_dir>）
    pub project_dir: &'static str,
    /// 全局路径（含 ~ 前缀，未展开）；None = 该 agent 不支持全局安装
    pub global_dir: Option<&'static str>,
    /// 本机检测用的 home 下配置目录（不含 ~ 前缀，如 ".claude"）
    pub detect_dirs: &'static [&'static str],
    /// project_dir == ".agents/skills" 的一批，共享同一目录
    pub universal: bool,
}

/// 运行形态：内置条目经 config 覆盖后的解析结果。
/// 只保留安装路径解析需要的字段；展示名走 AgentSpec.name（wizard/web 均直接列注册表）。
#[derive(Clone, Debug)]
pub struct ResolvedAgent {
    pub project_dir: Option<PathBuf>, // 自定义 agent（仅 config 定义）没有项目路径
    pub global_dir: Option<PathBuf>,  // 已展开 ~；已被 [targets] 同名覆盖
}

// 移植规则（对照 src/agents.ts 的 agents 记录逐条转换）：
// - 键名 → id；displayName → name；skillsDir → project_dir；globalSkillsDir → global_dir（undefined → None）。
// - detectInstalled 里 existsSync 的 home 配置目录 → detect_dirs（取 home 相对部分；
//   用 env 变量的取默认值目录，如 CODEX_HOME → ".codex"；用 cwd 相对目录或非 home 绝对路径的忽略）。
// - universal = skillsDir == ".agents/skills"。
// - configHome（xdgConfig ?? ~/.config）按默认值 "~/.config" 展开。
// 按 id 字典序排列（展示稳定）。表首 "agents" 是我们自加的 universal 共享目录条目。
pub const AGENTS: &[AgentSpec] = &[
    AgentSpec {
        id: "adal",
        name: "AdaL",
        project_dir: ".adal/skills",
        global_dir: Some("~/.adal/skills"),
        detect_dirs: &[".adal"],
        universal: false,
    },
    AgentSpec {
        id: "agents",
        name: "Universal (.agents/skills)",
        project_dir: ".agents/skills",
        global_dir: Some("~/.agents/skills"),
        detect_dirs: &[".agents"],
        universal: true,
    },
    AgentSpec {
        id: "aider-desk",
        name: "AiderDesk",
        project_dir: ".aider-desk/skills",
        global_dir: Some("~/.aider-desk/skills"),
        detect_dirs: &[".aider-desk"],
        universal: false,
    },
    AgentSpec {
        id: "amp",
        name: "Amp",
        project_dir: ".agents/skills",
        global_dir: Some("~/.config/agents/skills"),
        detect_dirs: &[".config/amp"],
        universal: true,
    },
    AgentSpec {
        id: "antigravity",
        name: "Antigravity",
        project_dir: ".agents/skills",
        global_dir: Some("~/.gemini/antigravity/skills"),
        detect_dirs: &[".gemini/antigravity"],
        universal: true,
    },
    AgentSpec {
        id: "antigravity-cli",
        name: "Antigravity CLI",
        project_dir: ".agents/skills",
        global_dir: Some("~/.gemini/antigravity-cli/skills"),
        detect_dirs: &[".gemini/antigravity-cli"],
        universal: true,
    },
    AgentSpec {
        id: "astrbot",
        name: "AstrBot",
        project_dir: "data/skills",
        global_dir: Some("~/.astrbot/data/skills"),
        detect_dirs: &[".astrbot"],
        universal: false,
    },
    AgentSpec {
        id: "augment",
        name: "Augment",
        project_dir: ".augment/skills",
        global_dir: Some("~/.augment/skills"),
        detect_dirs: &[".augment"],
        universal: false,
    },
    AgentSpec {
        id: "autohand-code",
        name: "Autohand Code CLI",
        project_dir: ".autohand/skills",
        global_dir: Some("~/.autohand/skills"),
        detect_dirs: &[".autohand"],
        universal: false,
    },
    AgentSpec {
        id: "bob",
        name: "IBM Bob",
        project_dir: ".bob/skills",
        global_dir: Some("~/.bob/skills"),
        detect_dirs: &[".bob"],
        universal: false,
    },
    AgentSpec {
        id: "claude-code",
        name: "Claude Code",
        project_dir: ".claude/skills",
        global_dir: Some("~/.claude/skills"),
        detect_dirs: &[".claude"],
        universal: false,
    },
    AgentSpec {
        id: "cline",
        name: "Cline",
        project_dir: ".agents/skills",
        global_dir: Some("~/.agents/skills"),
        detect_dirs: &[".cline"],
        universal: true,
    },
    AgentSpec {
        id: "codearts-agent",
        name: "CodeArts Agent",
        project_dir: ".codeartsdoer/skills",
        global_dir: Some("~/.codeartsdoer/skills"),
        detect_dirs: &[".codeartsdoer"],
        universal: false,
    },
    AgentSpec {
        id: "codebuddy",
        name: "CodeBuddy",
        project_dir: ".codebuddy/skills",
        global_dir: Some("~/.codebuddy/skills"),
        detect_dirs: &[".codebuddy"],
        universal: false,
    },
    AgentSpec {
        id: "codemaker",
        name: "Codemaker",
        project_dir: ".codemaker/skills",
        global_dir: Some("~/.codemaker/skills"),
        detect_dirs: &[".codemaker"],
        universal: false,
    },
    AgentSpec {
        id: "codestudio",
        name: "Code Studio",
        project_dir: ".codestudio/skills",
        global_dir: Some("~/.codestudio/skills"),
        detect_dirs: &[".codestudio"],
        universal: false,
    },
    AgentSpec {
        id: "codex",
        name: "Codex",
        project_dir: ".agents/skills",
        global_dir: Some("~/.codex/skills"),
        detect_dirs: &[".codex"],
        universal: true,
    },
    AgentSpec {
        id: "command-code",
        name: "Command Code",
        project_dir: ".commandcode/skills",
        global_dir: Some("~/.commandcode/skills"),
        detect_dirs: &[".commandcode"],
        universal: false,
    },
    AgentSpec {
        id: "continue",
        name: "Continue",
        project_dir: ".continue/skills",
        global_dir: Some("~/.continue/skills"),
        detect_dirs: &[".continue"],
        universal: false,
    },
    AgentSpec {
        id: "cortex",
        name: "Cortex Code",
        project_dir: ".cortex/skills",
        global_dir: Some("~/.snowflake/cortex/skills"),
        detect_dirs: &[".snowflake/cortex"],
        universal: false,
    },
    AgentSpec {
        id: "crush",
        name: "Crush",
        project_dir: ".crush/skills",
        global_dir: Some("~/.config/crush/skills"),
        detect_dirs: &[".config/crush"],
        universal: false,
    },
    AgentSpec {
        id: "cursor",
        name: "Cursor",
        project_dir: ".agents/skills",
        global_dir: Some("~/.cursor/skills"),
        detect_dirs: &[".cursor"],
        universal: true,
    },
    AgentSpec {
        id: "deepagents",
        name: "Deep Agents",
        project_dir: ".agents/skills",
        global_dir: Some("~/.deepagents/agent/skills"),
        detect_dirs: &[".deepagents"],
        universal: true,
    },
    AgentSpec {
        id: "devin",
        name: "Devin for Terminal",
        project_dir: ".devin/skills",
        global_dir: Some("~/.config/devin/skills"),
        detect_dirs: &[".config/devin"],
        universal: false,
    },
    AgentSpec {
        id: "dexto",
        name: "Dexto",
        project_dir: ".agents/skills",
        global_dir: Some("~/.agents/skills"),
        detect_dirs: &[".dexto"],
        universal: true,
    },
    AgentSpec {
        id: "droid",
        name: "Droid",
        project_dir: ".factory/skills",
        global_dir: Some("~/.factory/skills"),
        detect_dirs: &[".factory"],
        universal: false,
    },
    AgentSpec {
        id: "eve",
        name: "Eve",
        project_dir: "agent/skills",
        global_dir: None,
        // 检测依赖 cwd 与 package.json 内容，无法以 home 目录判定
        detect_dirs: &[],
        universal: false,
    },
    AgentSpec {
        id: "firebender",
        name: "Firebender",
        project_dir: ".agents/skills",
        global_dir: Some("~/.firebender/skills"),
        detect_dirs: &[".firebender"],
        universal: true,
    },
    AgentSpec {
        id: "forgecode",
        name: "ForgeCode",
        project_dir: ".forge/skills",
        global_dir: Some("~/.forge/skills"),
        detect_dirs: &[".forge"],
        universal: false,
    },
    AgentSpec {
        id: "gemini-cli",
        name: "Gemini CLI",
        project_dir: ".agents/skills",
        global_dir: Some("~/.gemini/skills"),
        detect_dirs: &[".gemini"],
        universal: true,
    },
    AgentSpec {
        id: "github-copilot",
        name: "GitHub Copilot",
        project_dir: ".agents/skills",
        global_dir: Some("~/.copilot/skills"),
        detect_dirs: &[".copilot"],
        universal: true,
    },
    AgentSpec {
        id: "goose",
        name: "Goose",
        project_dir: ".goose/skills",
        global_dir: Some("~/.config/goose/skills"),
        detect_dirs: &[".config/goose"],
        universal: false,
    },
    AgentSpec {
        id: "grok",
        name: "Grok Build",
        project_dir: ".grok/skills",
        global_dir: Some("~/.grok/skills"),
        detect_dirs: &[".grok"],
        universal: false,
    },
    AgentSpec {
        id: "hermes-agent",
        name: "Hermes Agent",
        project_dir: ".hermes/skills",
        global_dir: Some("~/.hermes/skills"),
        detect_dirs: &[".hermes"],
        universal: false,
    },
    AgentSpec {
        id: "iflow-cli",
        name: "iFlow CLI",
        project_dir: ".iflow/skills",
        global_dir: Some("~/.iflow/skills"),
        detect_dirs: &[".iflow"],
        universal: false,
    },
    AgentSpec {
        id: "inference-sh",
        name: "inference.sh",
        project_dir: ".inferencesh/skills",
        global_dir: Some("~/.inferencesh/skills"),
        detect_dirs: &[".inferencesh"],
        universal: false,
    },
    AgentSpec {
        id: "jazz",
        name: "Jazz",
        project_dir: ".jazz/skills",
        global_dir: Some("~/.jazz/skills"),
        detect_dirs: &[".jazz"],
        universal: false,
    },
    AgentSpec {
        id: "junie",
        name: "Junie",
        project_dir: ".junie/skills",
        global_dir: Some("~/.junie/skills"),
        detect_dirs: &[".junie"],
        universal: false,
    },
    AgentSpec {
        id: "kilo",
        name: "Kilo Code",
        project_dir: ".kilocode/skills",
        global_dir: Some("~/.kilocode/skills"),
        detect_dirs: &[".kilocode"],
        universal: false,
    },
    AgentSpec {
        id: "kimchi",
        name: "Kimchi",
        project_dir: ".kimchi/skills",
        global_dir: Some("~/.config/kimchi/harness/skills"),
        detect_dirs: &[".config/kimchi"],
        universal: false,
    },
    AgentSpec {
        id: "kimi-code-cli",
        name: "Kimi Code CLI",
        project_dir: ".agents/skills",
        global_dir: Some("~/.agents/skills"),
        detect_dirs: &[".kimi-code", ".kimi"],
        universal: true,
    },
    AgentSpec {
        id: "kiro-cli",
        name: "Kiro CLI",
        project_dir: ".kiro/skills",
        global_dir: Some("~/.kiro/skills"),
        detect_dirs: &[".kiro"],
        universal: false,
    },
    AgentSpec {
        id: "kode",
        name: "Kode",
        project_dir: ".kode/skills",
        global_dir: Some("~/.kode/skills"),
        detect_dirs: &[".kode"],
        universal: false,
    },
    AgentSpec {
        id: "lingma",
        name: "Lingma",
        project_dir: ".lingma/skills",
        global_dir: Some("~/.lingma/skills"),
        detect_dirs: &[".lingma"],
        universal: false,
    },
    AgentSpec {
        id: "loaf",
        name: "Loaf",
        project_dir: ".agents/skills",
        global_dir: Some("~/.agents/skills"),
        detect_dirs: &[".loaf"],
        universal: true,
    },
    AgentSpec {
        id: "mcpjam",
        name: "MCPJam",
        project_dir: ".mcpjam/skills",
        global_dir: Some("~/.mcpjam/skills"),
        detect_dirs: &[".mcpjam"],
        universal: false,
    },
    AgentSpec {
        id: "minimax-code",
        name: "MiniMax Code",
        project_dir: ".minimax/skills",
        global_dir: Some("~/.minimax/skills"),
        detect_dirs: &[".minimax"],
        universal: false,
    },
    AgentSpec {
        id: "mistral-vibe",
        name: "Mistral Vibe",
        project_dir: ".vibe/skills",
        global_dir: Some("~/.vibe/skills"),
        detect_dirs: &[".vibe"],
        universal: false,
    },
    AgentSpec {
        id: "moxby",
        name: "Moxby",
        project_dir: ".moxby/skills",
        global_dir: Some("~/.moxby/skills"),
        detect_dirs: &[".moxby"],
        universal: false,
    },
    AgentSpec {
        id: "mux",
        name: "Mux",
        project_dir: ".mux/skills",
        global_dir: Some("~/.mux/skills"),
        detect_dirs: &[".mux"],
        universal: false,
    },
    AgentSpec {
        id: "neovate",
        name: "Neovate",
        project_dir: ".neovate/skills",
        global_dir: Some("~/.neovate/skills"),
        detect_dirs: &[".neovate"],
        universal: false,
    },
    AgentSpec {
        id: "ona",
        name: "Ona",
        project_dir: ".ona/skills",
        global_dir: Some("~/.ona/skills"),
        detect_dirs: &[".ona"],
        universal: false,
    },
    AgentSpec {
        id: "openclaw",
        name: "OpenClaw",
        project_dir: "skills",
        global_dir: Some("~/.openclaw/skills"),
        detect_dirs: &[".openclaw", ".clawdbot", ".moltbot"],
        universal: false,
    },
    AgentSpec {
        id: "opencode",
        name: "OpenCode",
        project_dir: ".agents/skills",
        global_dir: Some("~/.config/opencode/skills"),
        detect_dirs: &[".config/opencode"],
        universal: true,
    },
    AgentSpec {
        id: "openhands",
        name: "OpenHands",
        project_dir: ".openhands/skills",
        global_dir: Some("~/.openhands/skills"),
        detect_dirs: &[".openhands"],
        universal: false,
    },
    AgentSpec {
        id: "pi",
        name: "Pi",
        project_dir: ".pi/skills",
        global_dir: Some("~/.pi/agent/skills"),
        detect_dirs: &[".pi/agent"],
        universal: false,
    },
    AgentSpec {
        id: "pochi",
        name: "Pochi",
        project_dir: ".pochi/skills",
        global_dir: Some("~/.pochi/skills"),
        detect_dirs: &[".pochi"],
        universal: false,
    },
    AgentSpec {
        id: "posit-assistant",
        name: "Posit Assistant",
        project_dir: ".posit/assistant/skills",
        global_dir: Some("~/.posit/assistant/skills"),
        detect_dirs: &[".posit/assistant", ".positai"],
        universal: false,
    },
    AgentSpec {
        id: "promptscript",
        name: "PromptScript",
        project_dir: ".agents/skills",
        global_dir: None,
        // 检测只看 cwd（.promptscript / promptscript.yaml），无 home 目录可判
        detect_dirs: &[],
        universal: true,
    },
    AgentSpec {
        id: "qoder",
        name: "Qoder",
        project_dir: ".qoder/skills",
        global_dir: Some("~/.qoder/skills"),
        detect_dirs: &[".qoder"],
        universal: false,
    },
    AgentSpec {
        id: "qoder-cn",
        name: "Qoder CN",
        project_dir: ".qoder/skills",
        global_dir: Some("~/.qoder-cn/skills"),
        detect_dirs: &[".qoder-cn"],
        universal: false,
    },
    AgentSpec {
        id: "qwen-code",
        name: "Qwen Code",
        project_dir: ".qwen/skills",
        global_dir: Some("~/.qwen/skills"),
        detect_dirs: &[".qwen"],
        universal: false,
    },
    AgentSpec {
        id: "reasonix",
        name: "Reasonix",
        project_dir: ".reasonix/skills",
        global_dir: Some("~/.reasonix/skills"),
        detect_dirs: &[".reasonix"],
        universal: false,
    },
    AgentSpec {
        id: "replit",
        name: "Replit",
        project_dir: ".agents/skills",
        global_dir: Some("~/.config/agents/skills"),
        // 检测只看 cwd 的 .replit，无 home 目录可判
        detect_dirs: &[],
        universal: true,
    },
    AgentSpec {
        id: "roo",
        name: "Roo Code",
        project_dir: ".roo/skills",
        global_dir: Some("~/.roo/skills"),
        detect_dirs: &[".roo"],
        universal: false,
    },
    AgentSpec {
        id: "rovodev",
        name: "Rovo Dev",
        project_dir: ".rovodev/skills",
        global_dir: Some("~/.rovodev/skills"),
        detect_dirs: &[".rovodev"],
        universal: false,
    },
    AgentSpec {
        id: "tabnine-cli",
        name: "Tabnine CLI",
        project_dir: ".tabnine/agent/skills",
        global_dir: Some("~/.tabnine/agent/skills"),
        detect_dirs: &[".tabnine"],
        universal: false,
    },
    AgentSpec {
        id: "terramind",
        name: "Terramind",
        project_dir: ".terramind/skills",
        global_dir: Some("~/.terramind/skills"),
        detect_dirs: &[".terramind"],
        universal: false,
    },
    AgentSpec {
        id: "tinycloud",
        name: "Tinycloud",
        project_dir: ".tinycloud/skills",
        global_dir: Some("~/.tinycloud/skills"),
        detect_dirs: &[".tinycloud"],
        universal: false,
    },
    AgentSpec {
        id: "trae",
        name: "Trae",
        project_dir: ".trae/skills",
        global_dir: Some("~/.trae/skills"),
        detect_dirs: &[".trae"],
        universal: false,
    },
    AgentSpec {
        id: "trae-cn",
        name: "Trae CN",
        project_dir: ".trae/skills",
        global_dir: Some("~/.trae-cn/skills"),
        detect_dirs: &[".trae-cn"],
        universal: false,
    },
    AgentSpec {
        id: "universal",
        name: "Universal",
        project_dir: ".agents/skills",
        global_dir: Some("~/.config/agents/skills"),
        // 上游 detectInstalled 恒为 false（仅供选择，不做本机检测）
        detect_dirs: &[],
        universal: true,
    },
    AgentSpec {
        id: "warp",
        name: "Warp",
        project_dir: ".agents/skills",
        global_dir: Some("~/.agents/skills"),
        detect_dirs: &[".warp"],
        universal: true,
    },
    AgentSpec {
        id: "windsurf",
        name: "Windsurf",
        project_dir: ".windsurf/skills",
        global_dir: Some("~/.codeium/windsurf/skills"),
        detect_dirs: &[".codeium/windsurf"],
        universal: false,
    },
    AgentSpec {
        id: "zcode",
        name: "ZCode",
        project_dir: ".zcode/skills",
        global_dir: Some("~/.zcode/skills"),
        detect_dirs: &[".zcode"],
        universal: false,
    },
    AgentSpec {
        id: "zed",
        name: "Zed",
        project_dir: ".agents/skills",
        global_dir: Some("~/.agents/skills"),
        detect_dirs: &[".config/zed"],
        universal: true,
    },
    AgentSpec {
        id: "zencoder",
        name: "Zencoder",
        project_dir: ".zencoder/skills",
        global_dir: Some("~/.zencoder/skills"),
        detect_dirs: &[".zencoder"],
        universal: false,
    },
    AgentSpec {
        id: "zenflow",
        name: "Zenflow",
        project_dir: ".zencoder/skills",
        global_dir: Some("~/.zencoder/skills"),
        detect_dirs: &[".zencoder"],
        universal: false,
    },
];

pub fn all() -> &'static [AgentSpec] {
    AGENTS
}

fn expand_tilde(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(s)
}

/// 解析 agent：内置表命中 → 应用 config 覆盖；否则 config 自定义（仅全局）；否则 None。
/// overrides 即 Config::targets（文件里的 [targets] 条目，已展开 ~）。
pub fn resolve(id: &str, overrides: &BTreeMap<String, PathBuf>) -> Result<ResolvedAgent> {
    if let Some(spec) = AGENTS.iter().find(|a| a.id == id) {
        let global_dir = overrides
            .get(id)
            .cloned()
            .or_else(|| spec.global_dir.map(expand_tilde));
        return Ok(ResolvedAgent {
            project_dir: Some(PathBuf::from(spec.project_dir)),
            global_dir,
        });
    }
    if let Some(p) = overrides.get(id) {
        return Ok(ResolvedAgent {
            project_dir: None,
            global_dir: Some(p.clone()),
        });
    }
    Err(Error::UnknownTarget(id.to_string()))
}

pub fn is_installed_at(home: &Path, spec: &AgentSpec) -> bool {
    spec.detect_dirs.iter().any(|d| home.join(d).is_dir())
}

/// 返回检测到的内置 agent id（含 "agents" 若 ~/.agents 存在）
pub fn detect_installed_at(home: &Path) -> Vec<&'static str> {
    AGENTS
        .iter()
        .filter(|a| is_installed_at(home, a))
        .map(|a| a.id)
        .collect()
}

/// 生产入口：home 取 dirs::home_dir()，取不到返回空
pub fn detect_installed() -> Vec<&'static str> {
    match dirs::home_dir() {
        Some(home) => detect_installed_at(&home),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_nonempty() {
        let mut seen = std::collections::HashSet::new();
        for a in AGENTS {
            assert!(!a.id.is_empty() && !a.name.is_empty());
            assert!(seen.insert(a.id), "重复 id: {}", a.id);
            assert!(!a.project_dir.is_empty());
            assert!(!a.project_dir.starts_with('/'));
        }
    }

    #[test]
    fn universal_flag_matches_project_dir() {
        for a in AGENTS {
            assert_eq!(a.universal, a.project_dir == ".agents/skills", "{}", a.id);
        }
    }

    #[test]
    fn registry_is_complete() {
        // 移植自 vercel-labs/skills src/agents.ts，条目数与其一致（允许 ±2 漂移，移植时按实际校准）
        assert!(AGENTS.len() >= 60, "只有 {} 条，移植不完整", AGENTS.len());
        assert!(AGENTS.iter().any(|a| a.id == "claude-code"));
        assert!(AGENTS.iter().any(|a| a.id == "codex"));
        assert!(AGENTS.iter().any(|a| a.id == "cursor"));
        assert!(AGENTS.iter().any(|a| a.id == "opencode"));
        assert!(AGENTS.iter().any(|a| a.id == "agents")); // 我们自加的 universal 共享目录条目
    }

    #[test]
    fn resolve_builtin_and_override() {
        let a = resolve("claude-code", &Default::default()).unwrap();
        // project_dir 是 Option<PathBuf>，与 &str 直接比较无法编译，改为 as_deref 比较
        assert_eq!(
            a.project_dir.as_deref(),
            Some(std::path::Path::new(".claude/skills"))
        );
        assert!(a.global_dir.is_some());
        assert!(resolve("nonexistent-agent", &Default::default()).is_err());
    }

    #[test]
    fn detect_uses_config_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let claude = AGENTS.iter().find(|a| a.id == "claude-code").unwrap();
        assert!(!is_installed_at(tmp.path(), claude));
        std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();
        assert!(is_installed_at(tmp.path(), claude));
    }

    #[test]
    fn detect_installed_returns_detected_ids() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".codex")).unwrap();
        let ids = detect_installed_at(tmp.path());
        assert!(ids.contains(&"codex"));
        assert!(!ids.contains(&"claude-code"));
    }
}
