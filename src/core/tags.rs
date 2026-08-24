use super::error::{Error, Result};
use super::registry::{Install, Registry, TargetRec};

/// 覆盖式设置某条 install 的分类（同一技能的其他目标副本不受影响）。
pub fn set_tags(
    reg: &mut Registry,
    skill: &str,
    target: &TargetRec,
    tags: Vec<String>,
) -> Result<()> {
    let inst = reg
        .installs
        .iter_mut()
        .find(|i| i.skill == skill && &i.target == target)
        .ok_or_else(|| Error::NotInstalled(skill.into()))?;
    inst.tags = tags;
    Ok(())
}

pub fn filter_by_tag<'a>(reg: &'a Registry, tag: &str) -> Vec<&'a Install> {
    reg.installs
        .iter()
        .filter(|i| i.tags.iter().any(|t| t == tag))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::registry::Method;

    fn reg() -> Registry {
        let mut r = Registry {
            version: 1,
            ..Default::default()
        };
        for (skill, name) in [("a", "agents"), ("a", "claude"), ("b", "agents")] {
            r.installs.push(Install {
                skill: skill.into(),
                source: "github/o/r".into(),
                source_path: format!("skills/{skill}").into(),
                target: TargetRec::Global { name: name.into() },
                method: Method::Copy,
                commit: "c1".into(),
                tags: vec![],
                auto_update: None,
                installed_at: "t".into(),
            });
        }
        r
    }

    #[test]
    fn set_tags_on_one_install_only() {
        let mut r = reg();
        let t = TargetRec::Global {
            name: "agents".into(),
        };
        set_tags(&mut r, "a", &t, vec!["frontend".into(), "ui".into()]).unwrap();
        assert_eq!(r.find("a", &t).unwrap().tags, vec!["frontend", "ui"]);
        // 同名技能在 claude 的副本不受影响
        let claude = TargetRec::Global {
            name: "claude".into(),
        };
        assert!(r.find("a", &claude).unwrap().tags.is_empty());
    }

    #[test]
    fn filter_by_tag() {
        let mut r = reg();
        set_tags(
            &mut r,
            "a",
            &TargetRec::Global {
                name: "agents".into(),
            },
            vec!["frontend".into()],
        )
        .unwrap();
        set_tags(
            &mut r,
            "b",
            &TargetRec::Global {
                name: "agents".into(),
            },
            vec!["backend".into()],
        )
        .unwrap();
        let hits = super::filter_by_tag(&r, "frontend");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].skill, "a");
        assert!(super::filter_by_tag(&r, "不存在").is_empty());
    }

    #[test]
    fn set_tags_on_missing_install_errors() {
        let mut r = reg();
        let t = TargetRec::Global {
            name: "agents".into(),
        };
        assert!(set_tags(&mut r, "nope", &t, vec!["x".into()]).is_err());
    }
}
