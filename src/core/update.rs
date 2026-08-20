//! 两级更新引擎：仓库级（auto_update 控制是否 fetch）+ 副本级（安装策略覆盖）。
//! symlink 跟随仓库级策略；显式 update <skill> --target 无视配置强制更新。

use super::error::Result;
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
        Some(sel) => {
            if let Some(inst) = reg.find(&sel.skill, &sel.target) {
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
        }
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

/// 执行计划（非 dry-run）：pull 仓库 → copy 副本重复制 → 更新 registry commit 字段。
pub fn execute_plan(
    layout: &super::paths::Layout,
    cfg: &super::config::Config,
    reg: &mut Registry,
    plan: &Plan,
) -> Result<Vec<String>> {
    let mut done = Vec::new();
    for key in &plan.sources {
        let cache = layout.cache_dir(key);
        if let Some(new_commit) = super::git::fetch_and_reset(&cache)? {
            if let Some(src) = reg.sources.get_mut(key) {
                src.commit = new_commit.clone();
                src.fetched_at = chrono::Utc::now().to_rfc3339();
            }
            done.push(format!("仓库 {key} → {new_commit:.8}"));
        }
    }
    for d in plan.copies.iter().filter(|c| c.update) {
        let rec = reg.find(&d.skill, &d.target).unwrap().clone();
        let target = match &d.target {
            TargetRec::Global { name } => super::paths::Target::Global { name: name.clone() },
            TargetRec::Project { root } => super::paths::Target::Project { root: root.clone() },
        };
        let dest = target.install_dir(cfg)?.join(&d.skill);
        let src_dir = layout.cache_dir(&rec.source).join(&rec.source_path);
        if dest.exists() {
            std::fs::remove_dir_all(&dest)?;
        }
        super::cache::copy_dir(&src_dir, &dest)?;
        let commit = reg.sources[&rec.source].commit.clone();
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
            inst.commit = reg.sources[&inst.source].commit.clone();
        }
        done.push(format!("软连接 {name} 跟随仓库"));
    }
    reg.save(layout)?;
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(plan.copies[0].update, true); // 显式指定无视配置
        assert_eq!(plan.sources.len(), 1); // 且会拉仓库
    }
}
