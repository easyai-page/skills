use super::agents;
use super::config::Config;
use super::error::{Error, Result};
use std::fmt;
use std::path::PathBuf;

#[derive(Clone, PartialEq, Debug)]
pub enum Scope {
    /// 项目级：值为项目根，最终目录 = <root>/<agent.project_dir>
    Project(PathBuf),
    Global,
}

/// 安装目标 = agent × scope。agent 为内置注册表 id 或 config 自定义名。
#[derive(Clone, PartialEq, Debug)]
pub struct Target {
    pub agent: String,
    pub scope: Scope,
}

impl Target {
    /// 支持新语法 `<agent>@global` / `<agent>@project:<abs路径>`，
    /// 兼容旧 -t 写法 `global:<agent>` 与 `project:<abs>`（= universal agents + 项目根）。
    pub fn parse(s: &str) -> Result<Target> {
        if let Some(rest) = s.strip_prefix("global:") {
            if rest.is_empty() {
                return Err(Error::BadTarget(s.into()));
            }
            return Ok(Target {
                agent: rest.into(),
                scope: Scope::Global,
            });
        }
        if let Some(rest) = s.strip_prefix("project:") {
            let p = PathBuf::from(rest);
            if !p.is_absolute() {
                return Err(Error::BadTarget(s.into()));
            }
            return Ok(Target {
                agent: "agents".into(),
                scope: Scope::Project(p),
            });
        }
        let (agent, rest) = s
            .split_once('@')
            .ok_or_else(|| Error::BadTarget(s.into()))?;
        if agent.is_empty() {
            return Err(Error::BadTarget(s.into()));
        }
        match rest {
            "global" => Ok(Target {
                agent: agent.into(),
                scope: Scope::Global,
            }),
            _ => {
                let p = PathBuf::from(
                    rest.strip_prefix("project:")
                        .ok_or_else(|| Error::BadTarget(s.into()))?,
                );
                if !p.is_absolute() {
                    return Err(Error::BadTarget(s.into()));
                }
                Ok(Target {
                    agent: agent.into(),
                    scope: Scope::Project(p),
                })
            }
        }
    }

    pub fn install_dir(&self, cfg: &Config) -> Result<PathBuf> {
        // resolve 的 Err 即 UnknownTarget(self.agent)，直接透传
        let spec = agents::resolve(&self.agent, &cfg.targets)?;
        match &self.scope {
            Scope::Global => spec
                .global_dir
                .clone()
                .ok_or_else(|| Error::Msg(format!("agent {} 不支持全局安装", self.agent))),
            Scope::Project(root) => spec.project_dir.map(|rel| root.join(rel)).ok_or_else(|| {
                Error::Msg(format!(
                    "agent {} 没有项目安装目录（自定义 target 仅支持全局）",
                    self.agent
                ))
            }),
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.scope {
            Scope::Global => write!(f, "{}@global", self.agent),
            Scope::Project(root) => write!(f, "{}@project:{}", self.agent, root.display()),
        }
    }
}

#[derive(Clone)]
pub struct Layout {
    pub root: PathBuf,
}

impl Layout {
    pub fn new() -> Result<Layout> {
        // 空串视为未设置：否则 root="" 会把 registry/config 写进当前工作目录
        if let Ok(p) = std::env::var("SKILLS_HOME")
            && !p.is_empty()
        {
            return Ok(Layout { root: p.into() });
        }
        let home = dirs::home_dir().ok_or(Error::NoHome)?;
        Ok(Layout {
            root: home.join(".skills"),
        })
    }
    /// 跳过环境变量探测直接指定 root —— 仅测试用（生产路径一律走 new 的 SKILLS_HOME 探测）
    #[cfg(test)]
    pub fn at(root: PathBuf) -> Layout {
        Layout { root }
    }
    pub fn cache_dir(&self, key: &str) -> PathBuf {
        self.root.join(key)
    }
    pub fn registry_path(&self) -> PathBuf {
        self.root.join("registry.json")
    }
    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.toml")
    }
    pub fn state_path(&self) -> PathBuf {
        self.root.join("state.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_new_syntax() {
        assert_eq!(
            Target::parse("claude-code@global").unwrap(),
            Target {
                agent: "claude-code".into(),
                scope: Scope::Global
            }
        );
        let abs = if cfg!(windows) {
            "codex@project:C:\\work"
        } else {
            "codex@project:/work"
        };
        assert!(matches!(
            Target::parse(abs).unwrap().scope,
            Scope::Project(_)
        ));
    }

    #[test]
    fn parse_legacy_syntax_maps_to_universal() {
        // 旧 -t 写法兼容：project:<abs> = universal agent + 项目根
        assert_eq!(
            Target::parse("global:claude-code").unwrap(),
            Target {
                agent: "claude-code".into(),
                scope: Scope::Global
            }
        );
        let t = Target::parse(if cfg!(windows) {
            "project:C:\\work"
        } else {
            "project:/work"
        })
        .unwrap();
        assert_eq!(t.agent, "agents");
        assert!(matches!(t.scope, Scope::Project(_)));
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(Target::parse("agents").is_err());
        assert!(Target::parse("global:").is_err());
        assert!(Target::parse("@global").is_err());
        assert!(Target::parse("claude-code@project:./rel").is_err());
    }

    #[test]
    fn display_roundtrip() {
        let t = Target {
            agent: "codex".into(),
            scope: Scope::Global,
        };
        assert_eq!(Target::parse(&t.to_string()).unwrap(), t);
    }

    #[test]
    fn install_dir_project_per_agent() {
        let cfg = crate::core::config::Config::default();
        let t = Target {
            agent: "claude-code".into(),
            scope: Scope::Project(PathBuf::from("/tmp/proj")),
        };
        assert_eq!(
            t.install_dir(&cfg).unwrap(),
            PathBuf::from("/tmp/proj/.claude/skills")
        );
        let u = Target {
            agent: "agents".into(),
            scope: Scope::Project(PathBuf::from("/tmp/proj")),
        };
        assert_eq!(
            u.install_dir(&cfg).unwrap(),
            PathBuf::from("/tmp/proj/.agents/skills")
        );
    }

    #[test]
    fn install_dir_global_and_errors() {
        let cfg = crate::core::config::Config::default();
        let g = Target {
            agent: "codex".into(),
            scope: Scope::Global,
        };
        let dir = g.install_dir(&cfg).unwrap();
        assert!(dir.ends_with(".codex/skills") || dir.ends_with(".codex\\skills"));
        // 不支持全局的 agent（eve 无 global_dir）
        let eve = Target {
            agent: "eve".into(),
            scope: Scope::Global,
        };
        assert!(eve.install_dir(&cfg).is_err());
        // 未知 agent
        assert!(
            Target {
                agent: "nope".into(),
                scope: Scope::Global
            }
            .install_dir(&cfg)
            .is_err()
        );
    }

    #[test]
    fn skills_home_env_handling() {
        // 本进程内只有此测试读写 SKILLS_HOME（cli_smoke 在子进程设 env），单测试内顺序断言无竞态
        unsafe {
            std::env::set_var("SKILLS_HOME", "/tmp/skills-test-override");
        }
        assert_eq!(
            Layout::new().unwrap().root,
            PathBuf::from("/tmp/skills-test-override")
        );
        unsafe {
            std::env::set_var("SKILLS_HOME", "");
        }
        let l = Layout::new().unwrap();
        assert!(
            l.root.is_absolute() && l.root.ends_with(".skills"),
            "{:?}",
            l.root
        );
        unsafe {
            std::env::remove_var("SKILLS_HOME");
        }
    }

    #[test]
    fn layout_paths() {
        let l = Layout::at(PathBuf::from("/x/.skills"));
        assert_eq!(
            l.cache_dir("github/a/b"),
            PathBuf::from("/x/.skills/github/a/b")
        );
        assert_eq!(l.registry_path(), PathBuf::from("/x/.skills/registry.json"));
    }
}
