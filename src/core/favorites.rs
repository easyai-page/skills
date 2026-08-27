//! 技能收藏：只记录地址与功能描述的快照，不安装。收藏与安装是正交的两份记录。

use std::path::Path;

use super::cache;
use super::config::Config;
use super::error::{Error, Result};
use super::install;
use super::paths::{Layout, Target};
use super::registry::{FavSkill, Favorite, Install, Method, Registry, SourceRecord};
use super::source::SourceSpec;

fn to_fav_skill(e: &cache::SkillEntry) -> FavSkill {
    FavSkill {
        name: e.name.clone(),
        description: e.description.clone(),
        source_path: e.rel_path.clone(),
    }
}

/// 收藏 source：skills 为空 = 整仓全量覆盖快照；非空 = 指定技能 upsert（不动其他）。
/// 同时把 source 登记进 sources 表（or_insert，与 add 相同逻辑），让缓存目录有唯一权威登记处。
/// 失败（clone/扫描/校验）时 favorite 不落盘，由调用方决定何时 save。
pub fn bookmark(
    layout: &Layout,
    reg: &mut Registry,
    spec: &SourceSpec,
    skills: &[String],
) -> Result<(String, usize)> {
    let cached = cache::ensure_cached(layout, spec)?;
    let all = cache::scan_skills(&cached.path)?;
    let picked: Vec<FavSkill> = if skills.is_empty() {
        all.iter().map(to_fav_skill).collect()
    } else {
        let mut out = Vec::new();
        for name in skills {
            let entry = all
                .iter()
                .find(|e| &e.name == name)
                .ok_or_else(|| Error::Msg(format!("仓库中无技能 {name}")))?;
            out.push(to_fav_skill(entry));
        }
        out
    };
    reg.sources.entry(spec.key.clone()).or_insert(SourceRecord {
        url: spec.url.clone().unwrap_or_default(),
        commit: cached.commit.clone(),
        fetched_at: chrono::Utc::now().to_rfc3339(),
        auto_update: None,
    });
    let count = picked.len();
    let now = chrono::Utc::now().to_rfc3339();
    if reg.favorites.contains_key(&spec.key) && !skills.is_empty() {
        // upsert：只覆盖指定技能，不动同 source 下其他已收藏技能
        let fav = reg.favorites.get_mut(&spec.key).expect("刚检查过存在");
        for s in picked {
            match fav.skills.iter_mut().find(|f| f.name == s.name) {
                Some(slot) => *slot = s,
                None => fav.skills.push(s),
            }
        }
        fav.skills.sort_by(|a, b| a.name.cmp(&b.name));
        fav.commit = cached.commit.clone();
        fav.bookmarked_at = now;
    } else {
        reg.favorites.insert(
            spec.key.clone(),
            Favorite {
                url: spec.url.clone(),
                local_path: spec.local_path.clone(),
                commit: cached.commit.clone(),
                bookmarked_at: now,
                skills: picked,
            },
        );
    }
    Ok((spec.key.clone(), count))
}

/// 删除收藏：skills 为空删整条 source；非空只删列出的（遇未知名报错，已删的不回滚）。
/// 技能删光则级联删 source 条目。不动缓存、不动 installs。
pub fn unbookmark(reg: &mut Registry, source_key: &str, skills: &[String]) -> Result<usize> {
    if !reg.favorites.contains_key(source_key) {
        return Err(Error::NotBookmarked(source_key.into()));
    }
    if skills.is_empty() {
        let n = reg.favorites[source_key].skills.len();
        reg.favorites.remove(source_key);
        return Ok(n);
    }
    let mut removed = 0;
    for name in skills {
        let fav = reg.favorites.get_mut(source_key).expect("刚检查过存在");
        let before = fav.skills.len();
        fav.skills.retain(|s| &s.name != name);
        if fav.skills.len() == before {
            return Err(Error::NotBookmarked(format!(
                "{source_key} 中无技能 {name}"
            )));
        }
        removed += 1;
    }
    if reg.favorites[source_key].skills.is_empty() {
        reg.favorites.remove(source_key);
    }
    Ok(removed)
}

/// rm/install 的 source 入参解析：先精确匹配 favorites key（如 github/o/r），
/// 不中再走 parse_source 规范化（贴 URL / 本地路径也行）。
pub fn resolve_key(reg: &Registry, input: &str) -> Result<String> {
    if reg.favorites.contains_key(input) {
        return Ok(input.into());
    }
    if let Ok(spec) = super::source::parse_source(input)
        && reg.favorites.contains_key(&spec.key)
    {
        return Ok(spec.key);
    }
    Err(Error::NotBookmarked(input.into()))
}

/// 单技能仓库判定：整仓只有一个根级技能（source_path 为 "."）。
/// 列表/视图据此把用途挂到一级行、不展开二级。
pub fn is_single_skill_repo(fav: &Favorite) -> bool {
    fav.skills.len() == 1 && fav.skills[0].source_path == Path::new(".")
}

/// 从收藏安装一个技能：source_path 取自收藏快照（不重扫仓库）。
/// 缓存缺失时凭快照的 url/local_path 重建 SourceSpec 并 ensure_cached 自愈（仅缺失时）；
/// 自愈后 HEAD 可能前进，同步刷新 sources 记录与收藏快照的 commit。
/// Error::Conflict 原样上抛交由前端决策；调用方负责落盘 registry。
pub fn fav_install(
    layout: &Layout,
    cfg: &Config,
    reg: &mut Registry,
    source_key: &str,
    skill: &str,
    target: &Target,
    method: Method,
) -> Result<Install> {
    let (source_path, url, local_path) = {
        let fav = reg
            .favorites
            .get(source_key)
            .ok_or_else(|| Error::NotBookmarked(source_key.into()))?;
        let s = fav
            .skills
            .iter()
            .find(|s| s.name == skill)
            .ok_or_else(|| Error::NotBookmarked(format!("{source_key} 中无技能 {skill}")))?;
        (
            s.source_path.clone(),
            fav.url.clone(),
            fav.local_path.clone(),
        )
    };
    // 缓存自愈：git 源以 .git 存在为完整标准，本地源以目录存在为准
    let cache = layout.cache_dir(source_key);
    let intact = if url.is_some() {
        cache.join(".git").is_dir()
    } else {
        cache.is_dir()
    };
    if !intact {
        if url.is_none() && local_path.as_ref().is_some_and(|p| !p.is_dir()) {
            return Err(Error::Msg(format!(
                "本地源 {} 已不存在，无法重建缓存；请重新 fav 有效路径",
                local_path.unwrap_or_default().display()
            )));
        }
        let spec = SourceSpec {
            key: source_key.into(),
            url,
            local_path,
        };
        let cached = cache::ensure_cached(layout, &spec)?;
        let now = chrono::Utc::now().to_rfc3339();
        let src = reg
            .sources
            .entry(source_key.into())
            .or_insert(SourceRecord {
                url: spec.url.clone().unwrap_or_default(),
                commit: String::new(),
                fetched_at: now.clone(),
                auto_update: None,
            });
        src.commit = cached.commit.clone();
        src.fetched_at = now;
        if let Some(fav) = reg.favorites.get_mut(source_key) {
            fav.commit = cached.commit.clone();
        }
    }
    let commit = reg
        .sources
        .get(source_key)
        .map(|s| s.commit.clone())
        .unwrap_or_default();
    install::install_skill(
        layout,
        cfg,
        reg,
        source_key,
        skill,
        &source_path.to_string_lossy(),
        target,
        method,
        &commit,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::Config;
    use crate::core::install::COPY_MANIFEST;
    use crate::core::paths::Target;
    use crate::core::registry::Method;
    use crate::core::source::parse_source;
    use std::path::PathBuf;

    /// 本地双技能源（local 源每次 ensure_cached 都重拷，天然离线）
    fn setup() -> (tempfile::TempDir, Layout, Registry, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let layout = Layout::at(tmp.path().join(".skills"));
        let src = tmp.path().join("src");
        std::fs::create_dir_all(src.join("skills/alpha")).unwrap();
        std::fs::create_dir_all(src.join("skills/beta")).unwrap();
        std::fs::write(
            src.join("skills/alpha/SKILL.md"),
            "---\nname: alpha\ndescription: A 技能\n---\n",
        )
        .unwrap();
        std::fs::write(
            src.join("skills/beta/SKILL.md"),
            "---\nname: beta\ndescription: B 技能\n---\n",
        )
        .unwrap();
        let reg = Registry {
            version: 1,
            ..Default::default()
        };
        (tmp, layout, reg, src)
    }

    fn local_spec(src: &Path) -> SourceSpec {
        parse_source(src.to_str().unwrap()).unwrap()
    }

    #[test]
    fn bookmark_whole_repo_snapshots_all_skills() {
        let (_t, layout, mut reg, src) = setup();
        let (key, n) = bookmark(&layout, &mut reg, &local_spec(&src), &[]).unwrap();
        assert_eq!(key, "local/src");
        assert_eq!(n, 2);
        let fav = &reg.favorites["local/src"];
        assert_eq!(fav.skills.len(), 2);
        assert_eq!(fav.skills[0].name, "alpha");
        assert_eq!(fav.skills[0].description, "A 技能");
        assert_eq!(fav.skills[0].source_path, PathBuf::from("skills/alpha"));
        assert_eq!(fav.local_path.as_deref(), Some(src.as_path()));
        assert!(fav.url.is_none());
        // 收藏同时登记 sources（与 add 相同逻辑），update 体系能看到缓存
        assert!(reg.sources.contains_key("local/src"));
    }

    #[test]
    fn bookmark_single_skill_upserts_without_touching_others() {
        let (_t, layout, mut reg, src) = setup();
        bookmark(&layout, &mut reg, &local_spec(&src), &["alpha".into()]).unwrap();
        assert_eq!(reg.favorites["local/src"].skills.len(), 1);
        // 再收 beta：upsert 进同一条目，不动 alpha
        bookmark(&layout, &mut reg, &local_spec(&src), &["beta".into()]).unwrap();
        let fav = &reg.favorites["local/src"];
        assert_eq!(fav.skills.len(), 2);
        // 改掉 alpha 描述后重收 alpha：只刷新 alpha 条目
        std::fs::write(
            src.join("skills/alpha/SKILL.md"),
            "---\nname: alpha\ndescription: A2 技能\n---\n",
        )
        .unwrap();
        bookmark(&layout, &mut reg, &local_spec(&src), &["alpha".into()]).unwrap();
        let fav = &reg.favorites["local/src"];
        let alpha = fav.skills.iter().find(|s| s.name == "alpha").unwrap();
        assert_eq!(alpha.description, "A2 技能");
        assert_eq!(fav.skills.len(), 2);
    }

    #[test]
    fn rebookmark_whole_repo_overwrites_snapshot() {
        let (_t, layout, mut reg, src) = setup();
        bookmark(&layout, &mut reg, &local_spec(&src), &[]).unwrap();
        // 源里删掉 beta：整仓重收藏 = 覆盖快照（手动刷新手段）
        std::fs::remove_dir_all(src.join("skills/beta")).unwrap();
        let (_key, n) = bookmark(&layout, &mut reg, &local_spec(&src), &[]).unwrap();
        assert_eq!(n, 1);
        let fav = &reg.favorites["local/src"];
        assert_eq!(fav.skills.len(), 1);
        assert_eq!(fav.skills[0].name, "alpha");
    }

    #[test]
    fn bookmark_unknown_skill_errors_and_writes_nothing() {
        let (_t, layout, mut reg, src) = setup();
        let err = bookmark(&layout, &mut reg, &local_spec(&src), &["nope".into()]).unwrap_err();
        assert!(format!("{err}").contains("仓库中无技能 nope"), "{err}");
        assert!(reg.favorites.is_empty());
    }

    #[test]
    fn unbookmark_single_then_cascade_when_empty() {
        let (_t, layout, mut reg, src) = setup();
        bookmark(&layout, &mut reg, &local_spec(&src), &[]).unwrap();
        let n = unbookmark(&mut reg, "local/src", &["alpha".into()]).unwrap();
        assert_eq!(n, 1);
        assert_eq!(reg.favorites["local/src"].skills.len(), 1);
        // 删光技能：级联删 source 条目
        let n = unbookmark(&mut reg, "local/src", &["beta".into()]).unwrap();
        assert_eq!(n, 1);
        assert!(!reg.favorites.contains_key("local/src"));
    }

    #[test]
    fn unbookmark_whole_source_and_missing_errors() {
        let (_t, layout, mut reg, src) = setup();
        bookmark(&layout, &mut reg, &local_spec(&src), &[]).unwrap();
        let n = unbookmark(&mut reg, "local/src", &[]).unwrap();
        assert_eq!(n, 2);
        assert!(reg.favorites.is_empty());
        // 未收藏的 source / 不存在的技能名：NotBookmarked
        assert!(matches!(
            unbookmark(&mut reg, "local/src", &[]),
            Err(Error::NotBookmarked(_))
        ));
        bookmark(&layout, &mut reg, &local_spec(&src), &[]).unwrap();
        assert!(matches!(
            unbookmark(&mut reg, "local/src", &["nope".into()]),
            Err(Error::NotBookmarked(_))
        ));
        // 部分删除遇未知名：已删的不回滚（逐条语义与 remove 一致），但整体报错
        assert!(matches!(
            unbookmark(&mut reg, "local/src", &["alpha".into(), "nope".into()]),
            Err(Error::NotBookmarked(_))
        ));
        assert_eq!(reg.favorites["local/src"].skills.len(), 1);
    }

    #[test]
    fn resolve_key_accepts_exact_key_or_source_expression() {
        let (_t, layout, mut reg, src) = setup();
        bookmark(&layout, &mut reg, &local_spec(&src), &[]).unwrap();
        // 精确 key
        assert_eq!(resolve_key(&reg, "local/src").unwrap(), "local/src");
        // 可解析的 source 表达式（本地绝对路径）
        assert_eq!(
            resolve_key(&reg, src.to_str().unwrap()).unwrap(),
            "local/src"
        );
        // 未收藏
        assert!(matches!(
            resolve_key(&reg, "github/x/y"),
            Err(Error::NotBookmarked(_))
        ));
    }

    #[test]
    fn single_skill_repo_detection() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = Layout::at(tmp.path().join(".skills"));
        let mut reg = Registry {
            version: 1,
            ..Default::default()
        };
        // 根级 SKILL.md 的单技能仓库
        let solo = tmp.path().join("solo");
        std::fs::create_dir_all(&solo).unwrap();
        std::fs::write(
            solo.join("SKILL.md"),
            "---\nname: solo\ndescription: 单技能\n---\n",
        )
        .unwrap();
        bookmark(&layout, &mut reg, &local_spec(&solo), &[]).unwrap();
        assert!(is_single_skill_repo(&reg.favorites["local/solo"]));
        // 多技能仓库不是
        let (_t2, layout2, mut reg2, src2) = setup();
        bookmark(&layout2, &mut reg2, &local_spec(&src2), &[]).unwrap();
        assert!(!is_single_skill_repo(&reg2.favorites["local/src"]));
    }

    /// setup + Config（agents target 指向临时目录），返回可复制安装的环境
    fn setup_with_cfg() -> (tempfile::TempDir, Layout, Config, Registry, PathBuf) {
        let (tmp, layout, reg, src) = setup();
        let mut cfg = Config::default();
        cfg.targets
            .insert("agents".into(), tmp.path().join("global/agents"));
        (tmp, layout, cfg, reg, src)
    }

    #[test]
    fn fav_install_installs_from_snapshot() {
        let (tmp, layout, cfg, mut reg, src) = setup_with_cfg();
        bookmark(&layout, &mut reg, &local_spec(&src), &[]).unwrap();
        let rec = fav_install(
            &layout,
            &cfg,
            &mut reg,
            "local/src",
            "alpha",
            &Target::Global {
                name: "agents".into(),
            },
            Method::Copy,
        )
        .unwrap();
        let dest = tmp.path().join("global/agents/alpha");
        assert!(dest.join("SKILL.md").exists());
        assert!(dest.join(COPY_MANIFEST).exists(), "copy 副本必须带归属标识");
        assert_eq!(rec.source, "local/src");
        assert_eq!(reg.installs.len(), 1);
    }

    #[test]
    fn fav_install_unknown_favorite_or_skill_errors() {
        let (_t, layout, cfg, mut reg, src) = setup_with_cfg();
        bookmark(&layout, &mut reg, &local_spec(&src), &[]).unwrap();
        let t = Target::Global {
            name: "agents".into(),
        };
        assert!(matches!(
            fav_install(
                &layout,
                &cfg,
                &mut reg,
                "local/nope",
                "alpha",
                &t,
                Method::Copy
            ),
            Err(Error::NotBookmarked(_))
        ));
        assert!(matches!(
            fav_install(
                &layout,
                &cfg,
                &mut reg,
                "local/src",
                "nope",
                &t,
                Method::Copy
            ),
            Err(Error::NotBookmarked(_))
        ));
        assert!(reg.installs.is_empty());
    }

    #[test]
    fn fav_install_heals_missing_cache() {
        let (tmp, layout, cfg, mut reg, src) = setup_with_cfg();
        bookmark(&layout, &mut reg, &local_spec(&src), &[]).unwrap();
        // 缓存被手动删除：fav install 凭快照的 local_path 重建缓存
        std::fs::remove_dir_all(layout.cache_dir("local/src")).unwrap();
        fav_install(
            &layout,
            &cfg,
            &mut reg,
            "local/src",
            "beta",
            &Target::Global {
                name: "agents".into(),
            },
            Method::Copy,
        )
        .unwrap();
        assert!(tmp.path().join("global/agents/beta/SKILL.md").exists());
    }

    #[test]
    fn fav_install_errors_when_cache_and_local_source_both_gone() {
        let (_t, layout, cfg, mut reg, src) = setup_with_cfg();
        bookmark(&layout, &mut reg, &local_spec(&src), &[]).unwrap();
        std::fs::remove_dir_all(layout.cache_dir("local/src")).unwrap();
        std::fs::remove_dir_all(&src).unwrap(); // 本地源本身也没了，无法自愈
        let err = fav_install(
            &layout,
            &cfg,
            &mut reg,
            "local/src",
            "alpha",
            &Target::Global {
                name: "agents".into(),
            },
            Method::Copy,
        )
        .unwrap_err();
        assert!(format!("{err}").contains("本地源"), "{err}");
        assert!(reg.installs.is_empty());
    }

    #[test]
    fn fav_install_conflict_returns_decision_request() {
        let (_t, layout, cfg, mut reg, src) = setup_with_cfg();
        bookmark(&layout, &mut reg, &local_spec(&src), &[]).unwrap();
        let t = Target::Global {
            name: "agents".into(),
        };
        fav_install(
            &layout,
            &cfg,
            &mut reg,
            "local/src",
            "alpha",
            &t,
            Method::Copy,
        )
        .unwrap();
        let err = fav_install(
            &layout,
            &cfg,
            &mut reg,
            "local/src",
            "alpha",
            &t,
            Method::Copy,
        )
        .unwrap_err();
        assert!(matches!(err, Error::Conflict(_)), "{err}");
    }
}
