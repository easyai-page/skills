use super::error::{Error, Result};
use super::registry::{Install, Registry};

/// 覆盖式设置某技能的分类。标签绑定 (source, skill)，与安装目标无关——
/// 同一技能装到多个目标共享同一份标签；tags 为空时清除条目。
pub fn set_tags(reg: &mut Registry, source: &str, skill: &str, tags: Vec<String>) -> Result<()> {
    if !reg
        .installs
        .iter()
        .any(|i| i.source == source && i.skill == skill)
    {
        return Err(Error::NotInstalled(skill.into()));
    }
    let skills = reg.skill_tags.entry(source.to_string()).or_default();
    if tags.is_empty() {
        skills.remove(skill);
        if skills.is_empty() {
            reg.skill_tags.remove(source);
        }
    } else {
        skills.insert(skill.to_string(), tags);
    }
    Ok(())
}

pub fn filter_by_tag<'a>(reg: &'a Registry, tag: &str) -> Vec<&'a Install> {
    reg.installs
        .iter()
        .filter(|i| reg.tags_of(&i.source, &i.skill).iter().any(|t| t == tag))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::registry::{Method, TargetRec};

    fn reg() -> Registry {
        let mut r = Registry {
            version: 1,
            ..Default::default()
        };
        // a 同时装到 agents 与 claude（同来源）；b 装到 agents；
        // 另有同名 a 来自第二个来源，用于验证标签按 source 隔离
        for (source, skill, name) in [
            ("github/o/r", "a", "agents"),
            ("github/o/r", "a", "claude"),
            ("github/o/r", "b", "agents"),
            ("github/x/y", "a", "agents"),
        ] {
            r.installs.push(Install {
                skill: skill.into(),
                source: source.into(),
                source_path: format!("skills/{skill}").into(),
                target: TargetRec::Global { name: name.into() },
                method: Method::Copy,
                commit: "c1".into(),
                auto_update: None,
                installed_at: "t".into(),
            });
        }
        r
    }

    #[test]
    fn set_tags_binds_to_skill_regardless_of_target() {
        let mut r = reg();
        set_tags(
            &mut r,
            "github/o/r",
            "a",
            vec!["frontend".into(), "ui".into()],
        )
        .unwrap();
        // 同一技能装到多个目标，标签共享
        assert_eq!(r.tags_of("github/o/r", "a"), &["frontend", "ui"]);
        // 其他技能不受影响
        assert!(r.tags_of("github/o/r", "b").is_empty());
    }

    #[test]
    fn tags_are_scoped_by_source() {
        let mut r = reg();
        set_tags(&mut r, "github/o/r", "a", vec!["frontend".into()]).unwrap();
        // 同名技能来自不同来源，标签互不可见
        assert!(r.tags_of("github/x/y", "a").is_empty());
    }

    #[test]
    fn empty_tags_clears_entry() {
        let mut r = reg();
        set_tags(&mut r, "github/o/r", "a", vec!["frontend".into()]).unwrap();
        set_tags(&mut r, "github/o/r", "a", vec![]).unwrap();
        assert!(r.tags_of("github/o/r", "a").is_empty());
        // 清空后不留空壳条目
        let has_entry = r
            .skill_tags
            .get("github/o/r")
            .map(|m| m.contains_key("a"))
            .unwrap_or(false);
        assert!(!has_entry);
    }

    #[test]
    fn set_tags_on_uninstalled_skill_errors() {
        let mut r = reg();
        assert!(set_tags(&mut r, "github/o/r", "nope", vec!["x".into()]).is_err());
        assert!(set_tags(&mut r, "github/unknown/z", "a", vec!["x".into()]).is_err());
    }

    #[test]
    fn filter_by_tag_matches_via_skill_tags() {
        let mut r = reg();
        set_tags(&mut r, "github/o/r", "a", vec!["frontend".into()]).unwrap();
        let hits = super::filter_by_tag(&r, "frontend");
        // 标签在技能级：a 的两个目标副本都命中；另一来源的同名 a 不命中
        assert_eq!(hits.len(), 2);
        assert!(
            hits.iter()
                .all(|i| i.skill == "a" && i.source == "github/o/r")
        );
        assert!(super::filter_by_tag(&r, "不存在").is_empty());
    }
}
