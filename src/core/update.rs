//! 两级更新引擎：仓库级（auto_update 控制是否 fetch）+ 副本级（安装策略覆盖）。
//! symlink 跟随仓库级策略；显式 update <skill> --target 无视配置强制更新。

use super::error::{Error, Result};
use super::registry::{Install, Method, Registry, SourceRecord, TargetRec};

pub fn repo_should_update(src: &SourceRecord) -> bool {
    src.auto_update.unwrap_or(false)
}

pub fn copy_should_update(reg: &Registry, inst: &Install) -> bool {
    inst.auto_update
        .or(reg.sources.get(&inst.source).and_then(|s| s.auto_update))
        .unwrap_or(false)
}

pub struct Selection {
    pub skill: String,
    pub target: TargetRec,
}

#[derive(Default, Debug)]
pub struct Plan {
    pub sources: Vec<String>,      // 将执行 pull 的仓库 key
    pub symlinks: Vec<String>,     // 随仓库更新的 symlink 技能名
    pub copies: Vec<CopyDecision>, // 每个 copy 副本的决定（含跳过的，供 dry-run 展示）
    /// 显式 selection 未命中安装记录时的可区分提示（Some 时 execute_plan 返回 NotInstalled）
    pub missing: Option<String>,
}

#[derive(Debug)]
pub struct CopyDecision {
    pub skill: String,
    pub target: TargetRec,
    pub update: bool,
    pub reason: String,
}

/// 构建更新计划。selection = Some 时只针对该副本，且无视配置强制更新。
pub fn build_plan(reg: &Registry, selection: Option<&Selection>) -> Plan {
    let mut plan = Plan::default();
    let mut pull: Vec<String> = Vec::new();
    match selection {
        Some(sel) => match reg.find(&sel.skill, &sel.target) {
            Some(inst) => {
                pull.push(inst.source.clone());
                match inst.method {
                    Method::Symlink => plan.symlinks.push(inst.skill.clone()),
                    Method::Copy => plan.copies.push(CopyDecision {
                        skill: inst.skill.clone(),
                        target: sel.target.clone(),
                        update: true,
                        reason: "显式指定".into(),
                    }),
                }
            }
            None => {
                plan.missing = Some(format!("{} @ {:?}", sel.skill, sel.target));
            }
        },
        None => {
            for (key, src) in &reg.sources {
                if repo_should_update(src) {
                    pull.push(key.clone());
                }
            }
            for inst in &reg.installs {
                let repo_in_plan = pull.contains(&inst.source);
                match inst.method {
                    Method::Symlink => {
                        if repo_in_plan {
                            plan.symlinks.push(inst.skill.clone());
                        }
                    }
                    Method::Copy => {
                        let allowed = copy_should_update(reg, inst);
                        plan.copies.push(CopyDecision {
                            skill: inst.skill.clone(),
                            target: inst.target.clone(),
                            update: repo_in_plan && allowed,
                            reason: if !repo_in_plan {
                                "仓库不更新".into()
                            } else if !allowed {
                                "副本级/包级配置关闭".into()
                            } else {
                                "更新".into()
                            },
                        });
                    }
                }
            }
        }
    }
    pull.sort();
    pull.dedup();
    plan.sources = pull;
    plan
}

/// 执行计划（非 dry-run）：pull 仓库 → copy 副本原子替换 → 更新 registry commit 字段。
pub fn execute_plan(
    layout: &super::paths::Layout,
    cfg: &super::config::Config,
    reg: &mut Registry,
    plan: &Plan,
) -> Result<Vec<String>> {
    if let Some(missing) = &plan.missing {
        return Err(Error::NotInstalled(missing.clone()));
    }
    let mut done = Vec::new();
    for key in &plan.sources {
        let cache = layout.cache_dir(key);
        if !cache.join(".git").exists() {
            return Err(Error::Msg(format!(
                "source {key} 的缓存不是 git 仓库，没有远端可拉取；本地源请重新 add 对应本地路径刷新"
            )));
        }
        if let Some(new_commit) = super::git::fetch_and_reset(&cache)? {
            if let Some(src) = reg.sources.get_mut(key) {
                src.commit = new_commit.clone();
                src.fetched_at = chrono::Utc::now().to_rfc3339();
            }
            done.push(format!("仓库 {key} → {new_commit:.8}"));
        }
    }
    for d in plan.copies.iter().filter(|c| c.update) {
        let rec = reg
            .find(&d.skill, &d.target)
            .ok_or_else(|| Error::NotInstalled(format!("{} @ {:?}", d.skill, d.target)))?
            .clone();
        // 与 remove 相同的前置防线：记录校验 + 副本归属核验；无法确认归属则不删不更新。
        super::remove::validate_record(&rec)?;
        let target = match &d.target {
            TargetRec::Global { name } => super::paths::Target::Global { name: name.clone() },
            TargetRec::Project { root } => super::paths::Target::Project { root: root.clone() },
        };
        let dest_root = target.install_dir(cfg)?;
        let dest = dest_root.join(&d.skill);
        match std::fs::symlink_metadata(&dest) {
            Ok(meta) => super::remove::verify_copy_ownership(&dest, &meta, &rec)?,
            // 副本已被手动删除：无可保护的内容，走 staging 直接重建
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(Error::Io(err)),
        }
        let src_dir = layout.cache_dir(&rec.source).join(&rec.source_path);
        super::install::replace_copy_install(&src_dir, &dest_root, &dest)?;
        let commit = reg
            .sources
            .get(&rec.source)
            .ok_or_else(|| {
                Error::Mismatch(format!("安装记录引用的 source 未登记: {}", rec.source))
            })?
            .commit
            .clone();
        if let Some(mut_inst) = reg
            .installs
            .iter_mut()
            .find(|i| i.skill == d.skill && i.target == d.target)
        {
            mut_inst.commit = commit;
        }
        done.push(format!("副本 {} @ {:?} 已更新", d.skill, d.target));
    }
    for name in &plan.symlinks {
        for inst in reg
            .installs
            .iter_mut()
            .filter(|i| &i.skill == name && i.method == Method::Symlink)
        {
            let commit = reg
                .sources
                .get(&inst.source)
                .ok_or_else(|| {
                    Error::Mismatch(format!("安装记录引用的 source 未登记: {}", inst.source))
                })?
                .commit
                .clone();
            inst.commit = commit;
        }
        done.push(format!("软连接 {name} 跟随仓库"));
    }
    reg.save(layout)?;
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::Config;
    use crate::core::install::{self, COPY_MANIFEST};
    use crate::core::paths::{Layout, Target};
    use crate::core::remove::{self, RemoveOutcome};
    use std::path::Path;

    fn git(dir: &Path, args: &[&str]) {
        let st = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(st.success(), "git {args:?} 失败");
    }
    fn reg_with(source_auto: Option<bool>, installs: Vec<(Method, Option<bool>)>) -> Registry {
        let mut reg = Registry {
            version: 1,
            ..Default::default()
        };
        reg.sources.insert(
            "github/o/r".into(),
            SourceRecord {
                url: "https://github.com/o/r".into(),
                commit: "c1".into(),
                fetched_at: "2026-08-20T00:00:00Z".into(),
                auto_update: source_auto,
            },
        );
        for (i, (method, auto)) in installs.into_iter().enumerate() {
            reg.installs.push(Install {
                skill: format!("s{i}"),
                source: "github/o/r".into(),
                source_path: format!("skills/s{i}").into(),
                target: TargetRec::Global {
                    name: "agents".into(),
                },
                method,
                commit: "c1".into(),
                tags: vec![],
                auto_update: auto,
                installed_at: "2026-08-20T00:00:00Z".into(),
            });
        }
        reg
    }

    #[test]
    fn repo_update_follows_source_flag_default_false() {
        assert!(!repo_should_update(
            &reg_with(None, vec![]).sources["github/o/r"]
        ));
        assert!(repo_should_update(
            &reg_with(Some(true), vec![]).sources["github/o/r"]
        ));
    }

    #[test]
    fn copy_effective_flag_install_overrides_source() {
        let reg = reg_with(Some(true), vec![(Method::Copy, Some(false))]);
        assert!(!copy_should_update(&reg, &reg.installs[0])); // 副本级 false 盖住包级 true
        let reg = reg_with(Some(false), vec![(Method::Copy, Some(true))]);
        assert!(copy_should_update(&reg, &reg.installs[0])); // 副本级 true 盖住包级 false
        let reg = reg_with(Some(true), vec![(Method::Copy, None)]);
        assert!(copy_should_update(&reg, &reg.installs[0])); // 跟随包级
        let reg = reg_with(None, vec![(Method::Copy, None)]);
        assert!(!copy_should_update(&reg, &reg.installs[0])); // 默认 false
    }

    #[test]
    fn plan_respects_two_levels() {
        // 包级开：symlink 全更新；copy 里 s0 副本关、s1 跟随
        let reg = reg_with(
            Some(true),
            vec![
                (Method::Symlink, None),
                (Method::Copy, Some(false)),
                (Method::Copy, None),
            ],
        );
        let plan = build_plan(&reg, None);
        assert_eq!(plan.sources.len(), 1);
        let copy_decisions: Vec<_> = plan
            .copies
            .iter()
            .map(|c| (c.skill.clone(), c.update))
            .collect();
        assert_eq!(
            copy_decisions,
            vec![("s1".into(), false), ("s2".into(), true)]
        );
        assert_eq!(plan.symlinks, vec!["s0".to_string()]);
        // 包级关：仓库不 pull，一切不更新
        let reg2 = reg_with(
            Some(false),
            vec![(Method::Symlink, None), (Method::Copy, None)],
        );
        let plan2 = build_plan(&reg2, None);
        assert!(plan2.sources.is_empty());
        assert!(plan2.symlinks.is_empty());
        assert!(plan2.copies.iter().all(|c| !c.update));
    }

    #[test]
    fn explicit_skill_target_forces_update() {
        let reg = reg_with(Some(false), vec![(Method::Copy, Some(false))]);
        let sel = Selection {
            skill: "s0".into(),
            target: TargetRec::Global {
                name: "agents".into(),
            },
        };
        let plan = build_plan(&reg, Some(&sel));
        assert!(plan.copies[0].update); // 显式指定无视配置
        assert_eq!(plan.sources.len(), 1); // 且会拉仓库
    }

    /// 快速造一个已安装的副本（假缓存，无 .git，不经 fetch）。
    fn setup_installed(method: Method) -> (tempfile::TempDir, Layout, Config, Registry) {
        let tmp = tempfile::tempdir().unwrap();
        let layout = Layout::at(tmp.path().join(".skills"));
        let cache = layout.cache_dir("github/o/r");
        std::fs::create_dir_all(cache.join("skills/alpha")).unwrap();
        std::fs::write(
            cache.join("skills/alpha/SKILL.md"),
            "---\nname: alpha\ndescription: A\n---\nv1\n",
        )
        .unwrap();
        let mut cfg = Config::default();
        cfg.targets
            .insert("agents".into(), tmp.path().join("g/agents"));
        let mut reg = Registry {
            version: 1,
            ..Default::default()
        };
        reg.sources.insert(
            "github/o/r".into(),
            SourceRecord {
                url: "https://github.com/o/r".into(),
                commit: "c1".into(),
                fetched_at: "2026-08-20T00:00:00Z".into(),
                auto_update: None,
            },
        );
        install::install_skill(
            &layout,
            &cfg,
            &mut reg,
            "github/o/r",
            "alpha",
            "skills/alpha",
            &Target::Global {
                name: "agents".into(),
            },
            method,
            "c1",
        )
        .unwrap();
        (tmp, layout, cfg, reg)
    }

    /// 真 git 远端 + 浅克隆缓存，可走完 execute_plan 的 fetch 路径。返回初始 commit。
    fn setup_with_repo(method: Method) -> (tempfile::TempDir, Layout, Config, Registry, String) {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("work");
        let bare = tmp.path().join("bare.git");
        std::fs::create_dir_all(work.join("skills/alpha")).unwrap();
        std::fs::write(
            work.join("skills/alpha/SKILL.md"),
            "---\nname: alpha\ndescription: A\n---\nv1\n",
        )
        .unwrap();
        git(&work, &["init", "-b", "main"]);
        git(&work, &["add", "."]);
        git(
            &work,
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-m",
                "c1",
            ],
        );
        git(&work, &["clone", "--bare", ".", bare.to_str().unwrap()]);

        let layout = Layout::at(tmp.path().join(".skills"));
        let cache = layout.cache_dir("github/o/r");
        std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
        let url = format!("file://{}", bare.display());
        let c1 = crate::core::git::shallow_clone(&url, &cache).unwrap();

        let mut cfg = Config::default();
        cfg.targets
            .insert("agents".into(), tmp.path().join("g/agents"));
        let mut reg = Registry {
            version: 1,
            ..Default::default()
        };
        reg.sources.insert(
            "github/o/r".into(),
            SourceRecord {
                url,
                commit: c1.clone(),
                fetched_at: "2026-08-20T00:00:00Z".into(),
                auto_update: None,
            },
        );
        install::install_skill(
            &layout,
            &cfg,
            &mut reg,
            "github/o/r",
            "alpha",
            "skills/alpha",
            &Target::Global {
                name: "agents".into(),
            },
            method,
            &c1,
        )
        .unwrap();
        (tmp, layout, cfg, reg, c1)
    }

    fn update_plan_for_alpha() -> Plan {
        Plan {
            copies: vec![CopyDecision {
                skill: "alpha".into(),
                target: TargetRec::Global {
                    name: "agents".into(),
                },
                update: true,
                reason: "测试".into(),
            }],
            ..Default::default()
        }
    }

    #[test]
    fn copy_update_replaces_via_staging_and_stays_removable() {
        let (_t, layout, cfg, mut reg) = setup_installed(Method::Copy);
        let dest = cfg.targets["agents"].join("alpha");
        // 缓存源与记录演进到 c2
        std::fs::write(
            layout.cache_dir("github/o/r").join("skills/alpha/SKILL.md"),
            "---\nname: alpha\ndescription: A\n---\nv2\n",
        )
        .unwrap();
        reg.sources.get_mut("github/o/r").unwrap().commit = "c2".into();

        let done = execute_plan(&layout, &cfg, &mut reg, &update_plan_for_alpha()).unwrap();

        assert!(done.iter().any(|line| line.contains("副本 alpha")));
        assert!(
            std::fs::read_to_string(dest.join("SKILL.md"))
                .unwrap()
                .contains("v2")
        );
        // 关键：更新后的副本仍带归属标识，remove 不会 Mismatch 拒删
        assert!(dest.join(COPY_MANIFEST).is_file());
        assert_eq!(reg.installs[0].commit, "c2");
        // 原子提交：目标目录下无暂存/备份残留
        let leftovers: Vec<_> = std::fs::read_dir(&cfg.targets["agents"])
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(leftovers, vec![std::ffi::OsString::from("alpha")]);
        // 更新后的副本可被 remove 正常删除
        let outcome = remove::remove_install(
            &layout,
            &cfg,
            &mut reg,
            "alpha",
            &TargetRec::Global {
                name: "agents".into(),
            },
        )
        .unwrap();
        assert_eq!(outcome, RemoveOutcome::Removed);
        assert!(!dest.exists());
        assert!(reg.installs.is_empty());
    }

    #[test]
    fn copy_update_refused_when_ownership_marker_missing() {
        let (_t, layout, cfg, mut reg) = setup_installed(Method::Copy);
        let dest = cfg.targets["agents"].join("alpha");
        std::fs::remove_file(dest.join(COPY_MANIFEST)).unwrap();

        let err = execute_plan(&layout, &cfg, &mut reg, &update_plan_for_alpha()).unwrap_err();

        assert!(matches!(err, Error::Mismatch(_)));
        // 不删不更新：原目录内容与记录保持原状
        assert!(
            std::fs::read_to_string(dest.join("SKILL.md"))
                .unwrap()
                .contains("v1")
        );
        assert_eq!(reg.installs[0].commit, "c1");
    }

    #[test]
    fn copy_update_refused_when_dir_replaced_by_foreign_dir() {
        let (_t, layout, cfg, mut reg) = setup_installed(Method::Copy);
        let dest = cfg.targets["agents"].join("alpha");
        std::fs::remove_dir_all(&dest).unwrap();
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("user-file"), "mine").unwrap();

        let err = execute_plan(&layout, &cfg, &mut reg, &update_plan_for_alpha()).unwrap_err();

        assert!(matches!(err, Error::Mismatch(_)));
        assert_eq!(
            std::fs::read_to_string(dest.join("user-file")).unwrap(),
            "mine"
        );
        assert_eq!(reg.installs[0].commit, "c1");
    }

    #[test]
    fn execute_plan_copy_missing_record_returns_not_installed() {
        let (_t, layout, cfg, mut reg) = setup_installed(Method::Copy);
        // plan 与 registry 不匹配：此前这里 unwrap panic
        let plan = Plan {
            copies: vec![CopyDecision {
                skill: "alpha".into(),
                target: TargetRec::Global {
                    name: "claude".into(),
                },
                update: true,
                reason: "测试".into(),
            }],
            ..Default::default()
        };
        let err = execute_plan(&layout, &cfg, &mut reg, &plan).unwrap_err();
        assert!(matches!(err, Error::NotInstalled(_)));
        assert!(cfg.targets["agents"].join("alpha").exists());
    }

    #[test]
    fn explicit_selection_for_unknown_skill_is_distinguishable() {
        let (_t, layout, cfg, mut reg) = setup_installed(Method::Copy);
        let sel = Selection {
            skill: "nope".into(),
            target: TargetRec::Global {
                name: "agents".into(),
            },
        };
        let plan = build_plan(&reg, Some(&sel));
        assert!(plan.missing.is_some());
        assert!(plan.sources.is_empty() && plan.copies.is_empty() && plan.symlinks.is_empty());
        let err = execute_plan(&layout, &cfg, &mut reg, &plan).unwrap_err();
        assert!(matches!(err, Error::NotInstalled(_)));
    }

    #[test]
    fn local_source_update_returns_friendly_error() {
        let (_t, layout, cfg, mut reg) = setup_installed(Method::Copy); // 假缓存：无 .git
        let plan = Plan {
            sources: vec!["github/o/r".into()],
            ..Default::default()
        };
        let err = execute_plan(&layout, &cfg, &mut reg, &plan).unwrap_err();
        assert!(matches!(err, Error::Msg(_)));
        assert!(format!("{err}").contains("本地源"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn explicit_selection_forces_update_for_symlink_and_copy() {
        for method in [Method::Symlink, Method::Copy] {
            let (tmp, layout, cfg, mut reg, c1) = setup_with_repo(method);
            // 远端前进到 c2
            let work = tmp.path().join("work");
            let bare = tmp.path().join("bare.git");
            std::fs::write(work.join("skills/alpha/SKILL.md"), "v2-content\n").unwrap();
            git(&work, &["add", "-A"]);
            git(
                &work,
                &[
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "user.name=t",
                    "commit",
                    "-m",
                    "c2",
                ],
            );
            git(&work, &["push", bare.to_str().unwrap(), "main"]);

            let sel = Selection {
                skill: "alpha".into(),
                target: TargetRec::Global {
                    name: "agents".into(),
                },
            };
            let plan = build_plan(&reg, Some(&sel)); // auto_update 全为 None，显式强制
            assert!(plan.missing.is_none());
            execute_plan(&layout, &cfg, &mut reg, &plan).unwrap();

            let src_commit = reg.sources["github/o/r"].commit.clone();
            assert_ne!(src_commit, c1, "{method:?}: fetch 应推进 commit");
            assert_eq!(reg.installs[0].commit, src_commit);
            let dest = cfg.targets["agents"].join("alpha");
            assert_eq!(
                std::fs::read_to_string(dest.join("SKILL.md")).unwrap(),
                "v2-content\n",
                "{method:?}"
            );
            match method {
                Method::Copy => assert!(dest.join(COPY_MANIFEST).is_file()),
                Method::Symlink => {
                    assert!(dest.symlink_metadata().unwrap().file_type().is_symlink())
                }
            }
        }
    }

    #[test]
    fn fetch_without_change_does_not_touch_commit() {
        let (_t, layout, cfg, mut reg, c1) = setup_with_repo(Method::Copy);
        reg.sources.get_mut("github/o/r").unwrap().auto_update = Some(true);
        reg.installs[0].auto_update = Some(true);
        let plan = build_plan(&reg, None);
        assert_eq!(plan.sources, vec!["github/o/r".to_string()]);

        let done = execute_plan(&layout, &cfg, &mut reg, &plan).unwrap();

        // fetch 无变化：记录 commit 与 fetched_at 不抖动，也没有“仓库 →”条目
        let src = &reg.sources["github/o/r"];
        assert_eq!(src.commit, c1);
        assert_eq!(src.fetched_at, "2026-08-20T00:00:00Z");
        assert_eq!(reg.installs[0].commit, c1);
        assert!(!done.iter().any(|line| line.starts_with("仓库")));
        // 幂等重复制后副本仍完整且带标识
        let dest = cfg.targets["agents"].join("alpha");
        assert!(dest.join(COPY_MANIFEST).is_file());
        assert!(
            std::fs::read_to_string(dest.join("SKILL.md"))
                .unwrap()
                .contains("v1")
        );
    }
}
