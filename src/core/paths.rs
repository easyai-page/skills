use super::config::Config;
use super::error::{Error, Result};
use std::path::PathBuf;

#[derive(Clone, PartialEq, Debug)]
pub enum Target {
    Global { name: String },
    Project { root: PathBuf },
}

impl Target {
    pub fn parse(s: &str) -> Result<Target> {
        let (kind, rest) = s
            .split_once(':')
            .ok_or_else(|| Error::BadTarget(s.into()))?;
        match kind {
            "global" if !rest.is_empty() => Ok(Target::Global { name: rest.into() }),
            "project" => {
                let p = PathBuf::from(rest);
                if p.is_absolute() {
                    Ok(Target::Project { root: p })
                } else {
                    Err(Error::BadTarget(s.into()))
                }
            }
            _ => Err(Error::BadTarget(s.into())),
        }
    }

    pub fn install_dir(&self, cfg: &Config) -> Result<PathBuf> {
        match self {
            Target::Global { name } => cfg
                .targets
                .get(name)
                .cloned()
                .ok_or_else(|| Error::UnknownTarget(name.clone())),
            Target::Project { root } => Ok(root.join(".agents").join("skills")),
        }
    }
}

pub struct Layout {
    pub root: PathBuf,
}

impl Layout {
    pub fn new() -> Result<Layout> {
        let home = dirs::home_dir().ok_or(Error::NoHome)?;
        Ok(Layout {
            root: home.join(".skills"),
        })
    }
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_global_target() {
        let t = Target::parse("global:agents").unwrap();
        assert_eq!(
            t,
            Target::Global {
                name: "agents".into()
            }
        );
    }

    #[test]
    fn parse_project_target_requires_absolute() {
        assert!(Target::parse("project:./rel").is_err());
        let abs = if cfg!(windows) {
            "project:C:\\work"
        } else {
            "project:/work"
        };
        assert!(Target::parse(abs).is_ok());
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(Target::parse("agents").is_err());
        assert!(Target::parse("global:").is_err());
    }

    #[test]
    fn project_install_dir_is_dot_agents_skills() {
        let cfg = crate::core::config::Config::default();
        let t = Target::Project {
            root: PathBuf::from("/tmp/proj"),
        };
        assert_eq!(
            t.install_dir(&cfg).unwrap(),
            PathBuf::from("/tmp/proj").join(".agents").join("skills")
        );
    }

    #[test]
    fn global_install_dir_resolves_via_config() {
        let cfg = crate::core::config::Config::default();
        let t = Target::Global {
            name: "agents".into(),
        };
        let dir = t.install_dir(&cfg).unwrap();
        assert!(dir.ends_with(".agents/skills") || dir.ends_with(".agents\\skills"));
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
