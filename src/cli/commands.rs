use super::{Cli, Cmd, ConfigCmd, FavSub, MethodArg, TargetsCmd};
use crate::core::{
    cache,
    config::Config,
    error::{Error, Result},
    favorites, install,
    paths::{Layout, Target},
    registry::{Method, Registry, TargetRec},
    remove,
    source::parse_source,
    tags, update,
};

pub fn run(cli: Cli) -> Result<()> {
    let layout = Layout::new()?;
    // Config 分支只读写 config.toml 原文，不依赖 Config::load 反序列化结果；
    // 必须先于 load 分发：配置损坏时 config set 仍是自愈出口，否则任何写入
    // 非法值（如 web.port = 99999）都会让包括 config 在内的全部命令锁死。
    if let Some(Cmd::Config { sub }) = &cli.cmd {
        return run_config(&layout, sub);
    }
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
            let targets = resolve_targets(&target, global, &cfg)?;
            let method = resolve_method(method, &cfg);
            // 全部技能名先校验再开工，避免装到一半才报"仓库中无技能"
            for s in &picked {
                if !all.iter().any(|e| &e.name == s) {
                    return Err(Error::Msg(format!("仓库中无技能 {s}")));
                }
            }
            install_loop(
                &layout,
                &cfg,
                &mut reg,
                &picked,
                &targets,
                method,
                yes,
                &|reg: &mut Registry, s: &str, t: &Target| {
                    let entry = all.iter().find(|e| e.name == s).expect("刚校验过存在");
                    install::install_skill(
                        &layout,
                        &cfg,
                        reg,
                        &spec.key,
                        &entry.name,
                        &entry.rel_path.to_string_lossy(),
                        t,
                        method,
                        &cached.commit,
                    )?;
                    Ok(())
                },
            )?;
            reg.save(&layout)
        }
        Some(Cmd::Remove {
            skills,
            target,
            tag,
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
            // --tag 命中与显式 skill×target 可能重叠；去重避免重复删除/重复输出
            let mut unique: Vec<(String, TargetRec)> = Vec::with_capacity(doomed.len());
            for item in doomed {
                if !unique.contains(&item) {
                    unique.push(item);
                }
            }
            let mut failed = 0usize;
            for (s, t) in unique {
                match remove::remove_install(&layout, &cfg, &mut reg, &s, &t) {
                    Ok(remove::RemoveOutcome::Removed) => println!("已删除 {s} @ {t:?}"),
                    Ok(remove::RemoveOutcome::RecordOnly) => {
                        println!("磁盘已不存在，仅清记录: {s} @ {t:?}")
                    }
                    // 单项失败不中断批量删除，但必须汇总后以非零退出，不得静默吞错
                    Err(e) => {
                        eprintln!("删除失败 {s} @ {t:?}: {e}");
                        failed += 1;
                    }
                }
            }
            reg.save(&layout)?;
            if failed > 0 {
                return Err(Error::Msg(format!("{failed} 项删除失败（详见 stderr）")));
            }
            Ok(())
        }
        Some(Cmd::Update {
            skills,
            target,
            all,
            dry_run,
            force,
        }) => {
            let mut reg = Registry::load(&layout)?;
            // 参数配对校验：显式技能必须搭配 --target；--all 是显式全量更新，与二者互斥。
            // 此前 `update <skill>`（无 --target）静默吞参退化为全量、多 skill + --target
            // 静默丢弃后续 skill，均为不可接受的隐晦行为。
            if all && (!skills.is_empty() || target.is_some()) {
                return Err(Error::Msg(
                    "--all 是显式全量更新，不能与技能名/--target 同用".into(),
                ));
            }
            let plan = match (&skills[..], target.as_deref()) {
                ([], None) => update::build_plan(&reg, None),
                ([], Some(_)) => {
                    return Err(Error::Msg(
                        "--target 需搭配技能名：skills update <技能> --target <目标>".into(),
                    ));
                }
                ([_, ..], None) => {
                    return Err(Error::Msg(
                        "显式更新技能必须同时给出 --target（或不带技能名做全量更新）".into(),
                    ));
                }
                // 多 skill + --target：逐个构造 selection 并合并计划（单 skill 即特例）
                (named, Some(t)) => {
                    let rec = install::to_rec(&Target::parse(t)?);
                    let mut merged = update::Plan::default();
                    for s in named {
                        let sel = update::Selection {
                            skill: s.clone(),
                            target: rec.clone(),
                        };
                        let p = update::build_plan(&reg, Some(&sel));
                        if let Some(missing) = p.missing {
                            // 显式指定却未安装：dry-run 也必须明确报错，不再静默略过
                            return Err(Error::NotInstalled(missing));
                        }
                        merged.sources.extend(p.sources);
                        merged.symlinks.extend(p.symlinks);
                        merged.copies.extend(p.copies);
                    }
                    merged.sources.sort();
                    merged.sources.dedup();
                    merged
                }
            };
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
            let done = match update::execute_plan(&layout, &cfg, &mut reg, &plan, force) {
                Ok(done) => done,
                // 副本有本地修改：展示明细并确认后才以 force 重跑；放弃则不产生任何变更。
                // （归属核验类的 Mismatch 即使确认后重跑也会再次报错，错误信息会如实呈现原因）
                Err(Error::Mismatch(msg)) if !force => {
                    println!("{msg}");
                    let confirmed = dialoguer::Confirm::new()
                        .with_prompt("将覆盖上述本地修改，继续？")
                        .default(false)
                        .interact()
                        .map_err(|e| {
                            Error::Msg(format!(
                                "确认交互失败（{e}）；非交互环境请改用 skills update --force 明确覆盖"
                            ))
                        })?;
                    if !confirmed {
                        println!("已取消，未做任何修改");
                        return Ok(());
                    }
                    update::execute_plan(&layout, &cfg, &mut reg, &plan, true)?
                }
                Err(err) => return Err(err),
            };
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
            // clap ArgGroup 已保证 --on/--off/--inherit 恰好给一个（裸调直接拒绝，
            // 不会像以前那样静默把设置清成 None）；走到这里 else 即 --inherit。
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
                inst.auto_update = val;
            } else {
                return Err(Error::Msg(
                    "需指定 --source <包> 或 <技能> + --target".into(),
                ));
            }
            reg.save(&layout)
        }
        Some(Cmd::Fav { source, skill, sub }) => {
            let mut reg = Registry::load(&layout)?;
            match (sub, source) {
                (Some(FavSub::Rm { source, skill }), _) => {
                    let key = favorites::resolve_key(&reg, &source)?;
                    if skill.is_empty() {
                        let n = favorites::unbookmark(&mut reg, &key, &[])?;
                        reg.save(&layout)?;
                        println!("已删除收藏 {key}（{n} 个技能）");
                    } else {
                        let n = favorites::unbookmark(&mut reg, &key, &skill)?;
                        reg.save(&layout)?;
                        println!("已从 {key} 删除 {n} 个技能收藏");
                    }
                    Ok(())
                }
                (
                    Some(FavSub::Install {
                        source,
                        skill,
                        target,
                        global,
                        method,
                        yes,
                    }),
                    _,
                ) => {
                    let key = favorites::resolve_key(&reg, &source)?;
                    let picked: Vec<String> = if skill.is_empty() {
                        let fav = &reg.favorites[&key];
                        if fav.skills.len() == 1 {
                            vec![fav.skills[0].name.clone()]
                        } else {
                            // 从收藏的技能集里选（不重扫全仓）
                            let names: Vec<String> =
                                fav.skills.iter().map(|s| s.name.clone()).collect();
                            let idx = dialoguer::MultiSelect::new()
                                .with_prompt("选择要安装的技能")
                                .items(&names)
                                .interact()
                                .map_err(|e| Error::Msg(e.to_string()))?;
                            idx.into_iter().map(|i| names[i].clone()).collect()
                        }
                    } else {
                        skill
                    };
                    if picked.is_empty() {
                        println!("未选择技能，取消安装");
                        return Ok(());
                    }
                    let targets = resolve_targets(&target, global, &cfg)?;
                    let method = resolve_method(method, &cfg);
                    install_loop(
                        &layout,
                        &cfg,
                        &mut reg,
                        &picked,
                        &targets,
                        method,
                        yes,
                        &|reg: &mut Registry, s: &str, t: &Target| {
                            favorites::fav_install(&layout, &cfg, reg, &key, s, t, method)?;
                            Ok(())
                        },
                    )?;
                    reg.save(&layout)
                }
                (None, Some(source)) => {
                    let spec = parse_source(&source)?;
                    let (key, n) = favorites::bookmark(&layout, &mut reg, &spec, &skill)?;
                    reg.save(&layout)?;
                    println!("已收藏 {key}（{n} 个技能）");
                    Ok(())
                }
                (None, None) => {
                    if !skill.is_empty() {
                        return Err(Error::Msg(
                            "--skill 需搭配 source：skills fav <仓库> --skill <名>".into(),
                        ));
                    }
                    print_favorites(&reg);
                    Ok(())
                }
            }
        }
        Some(Cmd::Config { .. }) => unreachable!("config 分支已在 Config::load 之前分发"),
    }
}

/// `-g` 且未显式 --target 时的默认安装目标：配置里第一个可用 global target
/// （BTreeMap 按名字排序，内置默认下即 agents）。用户删光 target 时给明确错误。
fn default_global_target(cfg: &Config) -> Result<Target> {
    cfg.targets
        .keys()
        .next()
        .map(|name| Target::Global { name: name.clone() })
        .ok_or_else(|| {
            Error::Msg("没有可用的全局 target：请先 skills config targets add <name> <路径>".into())
        })
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

/// config set 对已知标量键做写入前校验：非法值直接拒绝，
/// 避免写出 Config::load 无法解析的配置把全部命令锁死。
fn validate_config_set(key: &str, value: &str) -> Result<()> {
    match key {
        "web.port" => value
            .parse::<u16>()
            .map(|_| ())
            .map_err(|_| Error::Msg(format!("web.port 必须是 0-65535 的整数，收到: {value:?}"))),
        "defaults.method" => match value {
            "symlink" | "copy" => Ok(()),
            _ => Err(Error::Msg(format!(
                "defaults.method 只接受 symlink 或 copy，收到: {value:?}"
            ))),
        },
        _ => Ok(()),
    }
}

/// 收藏的两级列表：一级 = 技能包；多技能仓库二级逐行列技能名 + 用途；
/// 单技能仓库（is_single_skill_repo）二级留空，用途直接挂在一级行。
fn print_favorites(reg: &Registry) {
    if reg.favorites.is_empty() {
        println!("（无收藏）");
        return;
    }
    for (key, fav) in &reg.favorites {
        let date: String = fav.bookmarked_at.chars().take(10).collect();
        // chars().take(7)：按字符截断，避免多字节 UTF-8 在字节边界切片 panic
        let commit_short: String = fav.commit.chars().take(7).collect();
        let meta = if fav.url.is_some() {
            format!("({commit_short}, 收藏于 {date})")
        } else {
            "(本地源)".to_string()
        };
        if favorites::is_single_skill_repo(fav) {
            println!("{key} — {}    {meta}", fav.skills[0].description);
            continue;
        }
        println!("{key}    {meta}");
        for (i, s) in fav.skills.iter().enumerate() {
            let branch = if i + 1 == fav.skills.len() {
                "└─"
            } else {
                "├─"
            };
            println!("  {branch} {} — {}", s.name, s.description);
        }
    }
}

fn run_config(layout: &Layout, sub: &ConfigCmd) -> Result<()> {
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
            validate_config_set(key, value)?;
            // 原配置可解析时，写入后也必须可解析（兜住 targets.x = 123 这类未知键破坏）；
            // 原配置已损坏时是自愈场景，只校验本次写入的键，允许逐个键修回来。
            let was_loadable = crate::core::config::validate_config_toml(
                &toml::to_string_pretty(&doc).map_err(|e| Error::Msg(e.to_string()))?,
            )
            .is_ok();
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
            cur.insert(parts[parts.len() - 1].to_string(), scalar(value));
            if was_loadable {
                let text = toml::to_string_pretty(&doc).map_err(|e| Error::Msg(e.to_string()))?;
                crate::core::config::validate_config_toml(&text).map_err(|e| {
                    Error::Msg(format!(
                        "拒绝写入 {key}={value:?}：会导致配置无法解析（{e}）"
                    ))
                })?;
            }
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
                    targets.insert(name.clone(), toml::Value::String(p.clone()));
                }
                TargetsCmd::Remove { name } => {
                    if let Some(t) = doc.get_mut("targets").and_then(|t| t.as_table_mut()) {
                        t.remove(name);
                    }
                }
            }
            save(&doc)
        }
    }
}

/// add 与 fav install 共用的目标解析：-t 列表 / -g（配置里第一个 global target）/ 裸默认（当前项目）。
fn resolve_targets(target: &[String], global: bool, cfg: &Config) -> Result<Vec<Target>> {
    if !target.is_empty() {
        return target.iter().map(|s| Target::parse(s)).collect();
    }
    if global {
        // -g：装进配置里第一个可用 global target（内置默认下即 agents）
        return Ok(vec![default_global_target(cfg)?]);
    }
    // 默认装进当前项目：<cwd>/.agents/skills（-g 才装全局）
    Ok(vec![Target::Project {
        root: std::env::current_dir()?,
    }])
}

fn resolve_method(method: Option<MethodArg>, cfg: &Config) -> Method {
    match method {
        Some(MethodArg::Copy) => Method::Copy,
        Some(MethodArg::Symlink) => Method::Symlink,
        None => cfg.default_method,
    }
}

/// 「逐技能逐目标安装 + Conflict 确认/跳过 + 逐条落盘」循环。
/// 逐条落盘的原因同 add 原注释：中途失败时 registry 与磁盘已写入的副本保持一致。
/// install_fn 返回 Ok=装成，Err(Conflict)=走确认，其余 Err 直接中断。
// 参数即一次安装批次的全部上下文（布局/配置/记录/技能集/目标集/方式/确认/动作），
// 打包成结构体只是挪位置，与 install_skill 同款保留平铺签名。
#[allow(clippy::too_many_arguments)]
fn install_loop(
    layout: &Layout,
    cfg: &Config,
    reg: &mut Registry,
    picked: &[String],
    targets: &[Target],
    method: Method,
    yes: bool,
    install_fn: &dyn Fn(&mut Registry, &str, &Target) -> Result<()>,
) -> Result<()> {
    for s in picked {
        for t in targets {
            let installed = match install_fn(reg, s, t) {
                Ok(()) => {
                    println!("已安装 {s} → {t:?} ({method:?})");
                    true
                }
                Err(Error::Conflict(p)) => {
                    if yes {
                        println!("跳过已存在: {p:?}");
                        false
                    } else {
                        let overwrite = dialoguer::Confirm::new()
                            .with_prompt(format!("{p:?} 已存在，覆盖？"))
                            .interact()
                            .map_err(|e| Error::Msg(e.to_string()))?;
                        if overwrite {
                            let rec = install::to_rec(t);
                            let _ = remove::remove_install(layout, cfg, reg, s, &rec);
                            install_fn(reg, s, t)?;
                            true
                        } else {
                            false
                        }
                    }
                }
                Err(e) => return Err(e),
            };
            if installed {
                // 逐条落盘（tmp+rename 原子写，代价低）：之后任一技能/目标安装失败
                // 或覆盖重装失败而提前返回时，registry 都与磁盘已写入的副本保持一致，
                // 不会留下 list/remove 都管不到的孤儿副本。
                reg.save(layout)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_global_target_picks_first_configured() {
        // 内置默认 agents < claude < codex（按名字排序），与简报“等价 global:agents”一致
        let cfg = Config::default();
        let t = default_global_target(&cfg).unwrap();
        assert_eq!(
            t,
            Target::Global {
                name: "agents".into()
            }
        );
    }

    #[test]
    fn default_global_target_errors_when_targets_empty() {
        let cfg = Config {
            targets: Default::default(),
            ..Config::default()
        };
        let err = default_global_target(&cfg).unwrap_err();
        assert!(format!("{err}").contains("没有可用的全局 target"), "{err}");
    }

    #[test]
    fn validate_config_set_rejects_bad_known_scalars() {
        assert!(validate_config_set("web.port", "99999").is_err());
        assert!(validate_config_set("web.port", "-1").is_err());
        assert!(validate_config_set("web.port", "abc").is_err());
        assert!(validate_config_set("web.port", "9000").is_ok());
        assert!(validate_config_set("defaults.method", "foo").is_err());
        assert!(validate_config_set("defaults.method", "symlink").is_ok());
        assert!(validate_config_set("defaults.method", "copy").is_ok());
        // 未知键不在此拦截（由写入前的整份配置解析校验兜底）
        assert!(validate_config_set("targets.x", "123").is_ok());
    }
}
