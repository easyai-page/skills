use super::{Cli, Cmd, ConfigCmd, MethodArg, TargetsCmd};
use crate::core::{
    cache,
    config::Config,
    error::{Error, Result},
    install,
    paths::{Layout, Target},
    registry::{Method, Registry, TargetRec},
    remove,
    source::parse_source,
    tags, update,
};

pub fn run(cli: Cli) -> Result<()> {
    let layout = Layout::new()?;
    let cfg = Config::load(&layout)?;
    match cli.cmd {
        None | Some(Cmd::Tui) => crate::tui::run(&layout, &cfg),
        Some(Cmd::Ui { port, no_open }) => {
            crate::web::run(&layout, port.unwrap_or(cfg.web_port), no_open)
        }
        Some(Cmd::List {
            tag,
            target,
            global,
        }) => {
            let reg = Registry::load(&layout)?;
            let t = target.as_deref().map(Target::parse).transpose()?;
            let rows: Vec<_> = reg
                .installs
                .iter()
                .filter(|i| tag.as_ref().map(|t| i.tags.contains(t)).unwrap_or(true))
                .filter(|i| match &t {
                    Some(t) => install::to_rec(t) == i.target,
                    None => !global || matches!(i.target, TargetRec::Global { .. }),
                })
                .collect();
            if rows.is_empty() {
                println!("（无已安装技能）");
            }
            for i in rows {
                println!(
                    "{}\t{:?}\t{:?}\ttags={:?}\tauto_update={:?}",
                    i.skill, i.method, i.target, i.tags, i.auto_update
                );
            }
            Ok(())
        }
        Some(Cmd::Add {
            source,
            skill,
            target,
            global,
            method,
            yes,
        }) => {
            let mut reg = Registry::load(&layout)?;
            let spec = parse_source(&source)?;
            let cached = cache::ensure_cached(&layout, &spec)?;
            if !cached.fresh {
                println!("已缓存 {}，复用（skills update 可更新）", spec.key);
            }
            reg.sources
                .entry(spec.key.clone())
                .or_insert(crate::core::registry::SourceRecord {
                    url: spec.url.clone().unwrap_or_default(),
                    commit: cached.commit.clone(),
                    fetched_at: chrono::Utc::now().to_rfc3339(),
                    auto_update: None,
                });
            let all = cache::scan_skills(&cached.path)?;
            let picked: Vec<_> = if skill.is_empty() {
                // 交互多选
                let names: Vec<String> = all.iter().map(|s| s.name.clone()).collect();
                let idx = dialoguer::MultiSelect::new()
                    .with_prompt("选择要安装的技能")
                    .items(&names)
                    .interact()
                    .map_err(|e| Error::Msg(e.to_string()))?;
                idx.into_iter().map(|i| all[i].name.clone()).collect()
            } else {
                skill
            };
            let targets: Vec<Target> = if target.is_empty() {
                let default = if global {
                    "global:agents"
                } else {
                    "global:agents"
                };
                vec![Target::parse(default)?]
            } else {
                target
                    .iter()
                    .map(|s| Target::parse(s))
                    .collect::<Result<_>>()?
            };
            let method = match method {
                Some(MethodArg::Copy) => Method::Copy,
                Some(MethodArg::Symlink) => Method::Symlink,
                None => cfg.default_method,
            };
            for s in &picked {
                let entry = all
                    .iter()
                    .find(|e| &e.name == s)
                    .ok_or_else(|| Error::Msg(format!("仓库中无技能 {s}")))?;
                for t in &targets {
                    match install::install_skill(
                        &layout,
                        &cfg,
                        &mut reg,
                        &spec.key,
                        &entry.name,
                        &entry.rel_path.to_string_lossy(),
                        t,
                        method,
                        &cached.commit,
                    ) {
                        Ok(_) => println!("已安装 {s} → {t:?} ({method:?})"),
                        Err(Error::Conflict(p)) => {
                            if yes {
                                println!("跳过已存在: {p:?}");
                            } else {
                                let overwrite = dialoguer::Confirm::new()
                                    .with_prompt(format!("{p:?} 已存在，覆盖？"))
                                    .interact()
                                    .map_err(|e| Error::Msg(e.to_string()))?;
                                if overwrite {
                                    let rec = install::to_rec(t);
                                    let _ =
                                        remove::remove_install(&layout, &cfg, &mut reg, s, &rec);
                                    install::install_skill(
                                        &layout,
                                        &cfg,
                                        &mut reg,
                                        &spec.key,
                                        &entry.name,
                                        &entry.rel_path.to_string_lossy(),
                                        t,
                                        method,
                                        &cached.commit,
                                    )?;
                                }
                            }
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
            reg.save(&layout)
        }
        Some(Cmd::Remove {
            skills,
            target,
            tag,
            yes: _,
        }) => {
            let mut reg = Registry::load(&layout)?;
            // 组装待删集合：显式 skills × targets，或按 tag 全删
            let mut doomed: Vec<(String, TargetRec)> = Vec::new();
            if let Some(tg) = tag {
                doomed.extend(
                    tags::filter_by_tag(&reg, &tg)
                        .iter()
                        .map(|i| (i.skill.clone(), i.target.clone())),
                );
            }
            for s in &skills {
                if target.is_empty() {
                    doomed.extend(
                        reg.installs
                            .iter()
                            .filter(|i| &i.skill == s)
                            .map(|i| (i.skill.clone(), i.target.clone())),
                    );
                } else {
                    for t in &target {
                        doomed.push((s.clone(), install::to_rec(&Target::parse(t)?)));
                    }
                }
            }
            for (s, t) in doomed {
                match remove::remove_install(&layout, &cfg, &mut reg, &s, &t) {
                    Ok(remove::RemoveOutcome::Removed) => println!("已删除 {s} @ {t:?}"),
                    Ok(remove::RemoveOutcome::RecordOnly) => {
                        println!("磁盘已不存在，仅清记录: {s} @ {t:?}")
                    }
                    Err(e) => eprintln!("跳过 {s}: {e}"),
                }
            }
            reg.save(&layout)
        }
        Some(Cmd::Update {
            skills,
            target,
            all: _,
            dry_run,
        }) => {
            let mut reg = Registry::load(&layout)?;
            let sel = match (skills.first(), &target) {
                (Some(s), Some(t)) => Some(update::Selection {
                    skill: s.clone(),
                    target: install::to_rec(&Target::parse(t)?),
                }),
                _ => None,
            };
            let plan = update::build_plan(&reg, sel.as_ref());
            if dry_run {
                println!("将拉取仓库: {:?}", plan.sources);
                println!("软连接跟随: {:?}", plan.symlinks);
                for c in &plan.copies {
                    println!(
                        "{} @ {:?}: {}（{}）",
                        c.skill,
                        c.target,
                        if c.update { "更新" } else { "跳过" },
                        c.reason
                    );
                }
                return Ok(());
            }
            let done = update::execute_plan(&layout, &cfg, &mut reg, &plan)?;
            for line in done {
                println!("{line}");
            }
            Ok(())
        }
        Some(Cmd::Tag {
            skill,
            tags: new_tags,
            target,
            remove,
        }) => {
            let mut reg = Registry::load(&layout)?;
            let rec = install::to_rec(&Target::parse(&target)?);
            let final_tags = if remove { vec![] } else { new_tags };
            tags::set_tags(&mut reg, &skill, &rec, final_tags)?;
            reg.save(&layout)
        }
        Some(Cmd::AutoUpdate {
            skill,
            target,
            source,
            on,
            off,
            inherit,
        }) => {
            let mut reg = Registry::load(&layout)?;
            let val = if on {
                Some(true)
            } else if off {
                Some(false)
            } else {
                None
            };
            if let Some(src) = source {
                // 包级
                let s = reg
                    .sources
                    .get_mut(&src)
                    .ok_or_else(|| Error::Msg(format!("未知来源 {src}")))?;
                s.auto_update = val;
            } else if let (Some(s), Some(t)) = (skill, target) {
                // 副本级
                let rec = install::to_rec(&Target::parse(&t)?);
                let inst = reg
                    .installs
                    .iter_mut()
                    .find(|i| i.skill == s && i.target == rec)
                    .ok_or_else(|| Error::NotInstalled(s.clone()))?;
                if inst.method == Method::Symlink && !inherit {
                    eprintln!("提示：{s} 为软连接安装，更新策略跟随技能包（--source 设置）");
                }
                inst.auto_update = if inherit { None } else { val };
            } else {
                return Err(Error::Msg(
                    "需指定 --source <包> 或 <技能> + --target".into(),
                ));
            }
            reg.save(&layout)
        }
        Some(Cmd::Config { sub }) => run_config(&layout, sub),
    }
}

/// 标量值推断：纯数字存 Integer、true/false 存 Boolean，其余存 String。
/// 否则 `config set web.port 9000` 会写成字符串，导致 Config::load 反序列化失败，锁死 CLI。
fn scalar(value: &str) -> toml::Value {
    if let Ok(i) = value.parse::<i64>() {
        return toml::Value::Integer(i);
    }
    if let Ok(b) = value.parse::<bool>() {
        return toml::Value::Boolean(b);
    }
    toml::Value::String(value.to_string())
}

fn run_config(layout: &Layout, sub: ConfigCmd) -> Result<()> {
    // 读写 config.toml 原文（保留用户注释尽量简单：整体重写）
    let path = layout.config_path();
    let mut doc: toml::Value = if path.exists() {
        toml::from_str(&std::fs::read_to_string(&path)?)?
    } else {
        toml::Value::Table(Default::default())
    };
    let save = |doc: &toml::Value| -> Result<()> {
        let text = toml::to_string_pretty(doc).map_err(|e| Error::Msg(e.to_string()))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, text)?;
        Ok(())
    };
    match sub {
        ConfigCmd::Get { key } => {
            let mut cur = &doc;
            for part in key.split('.') {
                cur = cur
                    .get(part)
                    .ok_or_else(|| Error::Msg(format!("配置项不存在: {key}")))?;
            }
            println!("{cur}");
            Ok(())
        }
        ConfigCmd::Set { key, value } => {
            let parts: Vec<&str> = key.split('.').collect();
            let mut cur = doc
                .as_table_mut()
                .ok_or_else(|| Error::Msg("config.toml 顶层不是表".into()))?;
            for part in &parts[..parts.len() - 1] {
                cur = cur
                    .entry(part.to_string())
                    .or_insert_with(|| toml::Value::Table(Default::default()))
                    .as_table_mut()
                    .ok_or_else(|| Error::Msg(format!("配置路径被非表值占用: {key}")))?;
            }
            cur.insert(parts[parts.len() - 1].to_string(), scalar(&value));
            save(&doc)
        }
        ConfigCmd::Targets { sub } => {
            match sub {
                TargetsCmd::Add { name, path: p } => {
                    let targets = doc
                        .as_table_mut()
                        .ok_or_else(|| Error::Msg("config.toml 顶层不是表".into()))?
                        .entry("targets".to_string())
                        .or_insert_with(|| toml::Value::Table(Default::default()))
                        .as_table_mut()
                        .ok_or_else(|| Error::Msg("config.targets 不是表".into()))?;
                    targets.insert(name, toml::Value::String(p));
                }
                TargetsCmd::Remove { name } => {
                    if let Some(t) = doc.get_mut("targets").and_then(|t| t.as_table_mut()) {
                        t.remove(&name);
                    }
                }
            }
            save(&doc)
        }
    }
}
