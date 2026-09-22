use super::ui;
use super::{Cli, Cmd, ConfigCmd, FavSub, MethodArg, TargetsCmd};
use crate::core::state;
use crate::core::{
    agents, cache,
    config::Config,
    error::{Error, Result},
    favorites, install,
    paths::{Layout, Scope, Target},
    registry::{Method, Registry, TargetRec},
    remove,
    source::parse_source,
    tags, update,
};
use crate::wizard;
use std::io::IsTerminal;

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
            all,
        }) => {
            let reg = Registry::load(&layout)?;
            let t = target.as_deref().map(Target::parse).transpose()?;
            let cwd = std::env::current_dir().ok();
            let rows: Vec<_> = reg
                .installs
                .iter()
                .filter(|i| {
                    tag.as_ref()
                        .map(|t| reg.tags_of(&i.source, &i.skill).contains(t))
                        .unwrap_or(true)
                })
                .filter(|i| match &t {
                    Some(t) => install::to_rec(t) == i.target,
                    None => {
                        if global {
                            matches!(i.target, TargetRec::Global { .. })
                        } else if all {
                            true
                        } else {
                            match &cwd {
                                Some(d) => i.is_visible_in_dir(d),
                                None => true,
                            }
                        }
                    }
                })
                .collect();
            if rows.is_empty() {
                println!("（无已安装技能）");
            }
            for i in rows {
                println!(
                    "{}\t{:?}\t{}\ttags={:?}\tauto_update={:?}",
                    i.skill,
                    i.method,
                    i.target.to_target(),
                    reg.tags_of(&i.source, &i.skill),
                    i.auto_update
                );
            }
            Ok(())
        }
        Some(Cmd::Add {
            source,
            skill,
            agent,
            target,
            global,
            method,
            yes,
            all,
        }) => {
            let mut reg = Registry::load(&layout)?;
            let spec = parse_source(&source)?;
            let sp = ui::spinner("正在获取技能包…");
            let cached = match cache::ensure_cached(&layout, &spec) {
                Ok(c) => {
                    sp.finish_and_clear();
                    c
                }
                Err(e) => {
                    sp.finish_and_clear();
                    return Err(e);
                }
            };
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
            let sp = ui::spinner("正在扫描技能…");
            let all_skills = match cache::scan_skills(&cached.path) {
                Ok(s) => {
                    sp.finish_and_clear();
                    s
                }
                Err(e) => {
                    sp.finish_and_clear();
                    return Err(e);
                }
            };
            if all_skills.is_empty() {
                return Err(Error::Msg("仓库中没有可安装的技能".into()));
            }
            println!("{}", ui::ok(format!("发现 {} 个技能", all_skills.len())));
            let all_names: Vec<String> = all_skills.iter().map(|s| s.name.clone()).collect();
            // 第 1 步：技能。--all / -s '*' → 全部；-s 列表 → 校验后用之（显式 -s 优先于 -y 默认）；
            // 否则 TTY 弹 pick_skills（ESC/Ctrl+C = 取消），非 TTY → 全部（修掉旧版管道下弹窗报错的不对称）
            let picked: Vec<String> = if all || skill.iter().any(|s| s == "*") {
                all_names.clone()
            } else if !skill.is_empty() {
                resolve_skill_flag(&skill, &all_names)?
            } else if yes {
                all_names.clone()
            } else if std::io::stdin().is_terminal() {
                let cwd = std::env::current_dir().unwrap_or_default();
                let choices: Vec<wizard::SkillChoice> = all_skills
                    .iter()
                    .map(|e| {
                        let installed = reg
                            .installs
                            .iter()
                            .any(|i| i.skill == e.name && i.is_visible_in_dir(&cwd))
                            || cwd.join(".agents/skills").join(&e.name).exists()
                            || cwd.join(".claude/skills").join(&e.name).exists();
                        wizard::SkillChoice {
                            name: e.name.clone(),
                            description: e.description.clone(),
                            group: e.group.clone(),
                            installed,
                        }
                    })
                    .collect();
                match wizard::pick_skills(&choices)? {
                    Some(p) => p,
                    None => {
                        println!("已取消安装");
                        return Ok(());
                    }
                }
            } else {
                all_names.clone()
            };
            // 第 2~4 步：agent → scope → method（与 fav install 共用）
            let (targets, method) =
                resolve_install_plan(&layout, &agent, &target, global, all, yes, method, &cfg)?;
            if targets.is_empty() {
                println!("已取消安装");
                return Ok(());
            }
            println!(
                "将安装 {} 个技能 → {} 个目标（{}）",
                picked.len(),
                targets.len(),
                ui::method_label(method)
            );
            install_loop(
                &layout,
                &cfg,
                &mut reg,
                &picked,
                &targets,
                method,
                // --all 隐含 -y 语义：冲突跳过，不弹确认
                yes || all,
                &|reg: &mut Registry, s: &str, t: &Target| {
                    let entry = all_skills
                        .iter()
                        .find(|e| e.name == s)
                        .expect("刚校验过存在");
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
                    Ok(remove::RemoveOutcome::Removed) => {
                        println!("已删除 {s} @ {}", t.to_target())
                    }
                    Ok(remove::RemoveOutcome::RecordOnly) => {
                        println!("磁盘已不存在，仅清记录: {s} @ {}", t.to_target())
                    }
                    // 单项失败不中断批量删除，但必须汇总后以非零退出，不得静默吞错
                    Err(e) => {
                        eprintln!("删除失败 {s} @ {}: {e}", t.to_target());
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
                        "{} @ {}: {}（{}）",
                        c.skill,
                        c.target.to_target(),
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
                    let confirmed = inquire::Confirm::new("将覆盖上述本地修改，继续？")
                        .with_default(false)
                        .prompt()
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
            source,
            remove,
        }) => {
            let mut reg = Registry::load(&layout)?;
            let src = resolve_tag_source(&reg, &skill, source)?;
            let final_tags = if remove { vec![] } else { new_tags };
            tags::set_tags(&mut reg, &src, &skill, final_tags)?;
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
                        agent,
                        target,
                        global,
                        method,
                        yes,
                        all,
                    }),
                    _,
                ) => {
                    let key = favorites::resolve_key(&reg, &source)?;
                    // 第 1 步：技能（候选 = 收藏集，不重扫全仓）。--all / -s '*' → 全部；
                    // 显式 -s 列表用之（未收藏名由 fav_install 的 NotBookmarked 在逐条循环里拦截）；
                    // 否则 TTY 弹 pick_skills（单技能直接装，不弹窗），非 TTY → 全部
                    let fav_names: Vec<String> = reg.favorites[&key]
                        .skills
                        .iter()
                        .map(|s| s.name.clone())
                        .collect();
                    let picked: Vec<String> = if all || skill.iter().any(|s| s == "*") {
                        fav_names.clone()
                    } else if !skill.is_empty() {
                        skill
                    } else if yes || !std::io::stdin().is_terminal() {
                        fav_names.clone()
                    } else {
                        let cwd = std::env::current_dir().unwrap_or_default();
                        let choices: Vec<wizard::SkillChoice> = reg.favorites[&key]
                            .skills
                            .iter()
                            .map(|s| {
                                let installed = reg
                                    .installs
                                    .iter()
                                    .any(|i| i.skill == s.name && i.is_visible_in_dir(&cwd))
                                    || cwd.join(".agents/skills").join(&s.name).exists()
                                    || cwd.join(".claude/skills").join(&s.name).exists();
                                wizard::SkillChoice {
                                    name: s.name.clone(),
                                    description: s.description.clone(),
                                    group: None,
                                    installed,
                                }
                            })
                            .collect();
                        match wizard::pick_skills(&choices)? {
                            Some(p) => p,
                            None => {
                                println!("已取消安装");
                                return Ok(());
                            }
                        }
                    };
                    if picked.is_empty() {
                        println!("已取消安装");
                        return Ok(());
                    }
                    // 第 2~4 步：agent → scope → method（与 add 共用）
                    let (targets, method) = resolve_install_plan(
                        &layout, &agent, &target, global, all, yes, method, &cfg,
                    )?;
                    if targets.is_empty() {
                        println!("已取消安装");
                        return Ok(());
                    }
                    println!(
                        "将安装 {} 个技能 → {} 个目标（{}）",
                        picked.len(),
                        targets.len(),
                        ui::method_label(method)
                    );
                    install_loop(
                        &layout,
                        &cfg,
                        &mut reg,
                        &picked,
                        &targets,
                        method,
                        yes || all,
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

/// tag 命令的来源解析：标签绑定 (source, skill)，裸技能名只在来源唯一时可用，
/// 多来源同名技能必须显式 --source，不做隐晦的"取第一个"。
fn resolve_tag_source(reg: &Registry, skill: &str, source: Option<String>) -> Result<String> {
    let mut sources: Vec<&str> = reg
        .installs
        .iter()
        .filter(|i| i.skill == skill)
        .map(|i| i.source.as_str())
        .collect();
    sources.sort();
    sources.dedup();
    match source {
        Some(s) => {
            if sources.contains(&s.as_str()) {
                Ok(s)
            } else {
                Err(Error::NotInstalled(format!("{skill}（来源 {s}）")))
            }
        }
        None => match sources[..] {
            [] => Err(Error::NotInstalled(skill.into())),
            [one] => Ok(one.to_string()),
            _ => Err(Error::Msg(format!(
                "技能 {skill} 来自多个来源（{}），请用 --source 指定",
                sources.join(", ")
            ))),
        },
    }
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

/// -s/--skill 列表解析：'*' = 全部；其余名字逐一预校验，任一不存在即报错
/// （保持「全部技能名先校验再开工」语义，避免装到一半才报"仓库中无技能"）。
fn resolve_skill_flag(flag: &[String], all_names: &[String]) -> Result<Vec<String>> {
    if flag.iter().any(|s| s == "*") {
        return Ok(all_names.to_vec());
    }
    for s in flag {
        if !all_names.contains(s) {
            return Err(Error::Msg(format!("仓库中无技能 {s}")));
        }
    }
    Ok(flag.to_vec())
}

/// -y 的 agent 默认：检测到的内置 agent；一个都检测不到时退回 ["agents"]
fn yes_agents(detected: &[&str]) -> Vec<String> {
    if detected.is_empty() {
        vec!["agents".to_string()]
    } else {
        detected.iter().map(|s| s.to_string()).collect()
    }
}

/// add 与 fav install 共用的第 2~4 步：agent → scope → method，返回展开去重后的
/// 目标列表与安装方式。返回空目标列表 = 用户在弹窗里取消（agent/scope/方式步 ESC/Ctrl+C），
/// 调用方视为取消安装。
/// -t 旧写法直达 targets，不弹 agent/scope/method；--all / -y / 非 TTY 一律不弹窗。
// 参数即一次安装决策的全部输入（agent/target 列表、三个开关、--method、配置），
// 打包成结构体只是挪位置，保留平铺签名。
#[allow(clippy::too_many_arguments)]
fn resolve_install_plan(
    layout: &Layout,
    agent: &[String],
    target: &[String],
    global: bool,
    all: bool,
    yes: bool,
    method: Option<MethodArg>,
    cfg: &Config,
) -> Result<(Vec<Target>, Method)> {
    if !target.is_empty() && !agent.is_empty() {
        return Err(Error::Msg("-t 与 -a 只能给一个".into()));
    }
    // -t 兼容旧写法：直接 parse 成 targets 跳到安装，后续步骤不弹窗
    if !target.is_empty() {
        let targets = target
            .iter()
            .map(|s| Target::parse(s))
            .collect::<Result<Vec<_>>>()?;
        return Ok((targets, resolve_method(method, cfg)));
    }
    let no_prompt = all || yes || !std::io::stdin().is_terminal();
    // 纯兜底路径（非 TTY 且未给 -y/--all）给用户一行引导，行为本身不变
    if !all && !yes && !std::io::stdin().is_terminal() {
        eprintln!("提示：非交互环境可使用 -y / -s / -a / -t / -g 参数控制安装行为");
    }
    // 第 2 步：agent。--all / -a '*' → 全内置 agent；-a 列表 → 全部先校验再开工；
    // -y → 检测到的（检测不到退回 agents）；否则 TTY 弹 pick_agents（记忆 > 检测 > agents 预选），非 TTY → agents
    let agent_ids: Vec<String> = if all || agent.iter().any(|a| a == "*") {
        agents::all().iter().map(|a| a.id.to_string()).collect()
    } else if !agent.is_empty() {
        for id in agent {
            agents::resolve(id, &cfg.targets)
                .map_err(|_| Error::Msg(format!("未知 agent {id}")))?;
        }
        agent.to_vec()
    } else if yes {
        yes_agents(&agents::detect_installed())
    } else if no_prompt {
        vec!["agents".to_string()]
    } else {
        let detected = agents::detect_installed();
        let remembered = state::load(layout).last_selected_agents;
        match wizard::pick_agents(&detected, &remembered)? {
            Some(ids) => {
                state::save_last_agents(layout, &ids);
                ids
            }
            None => return Ok((vec![], cfg.default_method)), // 取消
        }
    };
    // 第 3 步：scope。-g → Global；不弹窗路径 → 项目（cwd）；
    // 否则 pick_scope（内部对全不支持全局的 agent 组合直接返回项目，不弹窗；ESC/Ctrl+C = 取消）
    let scope = if global {
        Scope::Global
    } else if no_prompt {
        Scope::Project(std::env::current_dir()?)
    } else {
        match wizard::pick_scope(&agent_ids)? {
            Some(s) => s,
            None => return Ok((vec![], cfg.default_method)), // 取消
        }
    };
    // 第 4 步：method。--method → 用之；不弹窗路径 → cfg.default_method；否则 pick_method（ESC/Ctrl+C = 取消）
    let method = match method {
        Some(m) => resolve_method(Some(m), cfg),
        None if no_prompt => cfg.default_method,
        None => match wizard::pick_method()? {
            Some(m) => m,
            None => return Ok((vec![], cfg.default_method)), // 取消
        },
    };
    let targets = wizard::expand_targets(&agent_ids, &scope, cfg)?;
    Ok((targets, method))
}

fn resolve_method(method: Option<MethodArg>, cfg: &Config) -> Method {
    match method {
        Some(MethodArg::Copy) => Method::Copy,
        Some(MethodArg::Symlink) => Method::Symlink,
        None => cfg.default_method,
    }
}

/// 「逐技能逐目标安装 + Conflict 确认/跳过 + 逐条落盘 + 收尾摘要」循环。
/// 逐条落盘的原因不变：中途失败时 registry 与磁盘已写入的副本保持一致。
/// install_fn 返回 Ok=装成，Err(Conflict)=走确认（yes=跳过），其余 Err 中断。
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
    let total = picked.len() * targets.len();
    let pb = ui::spinner(format!("正在安装（共 {total} 条）…"));
    let mut installed = 0usize;
    let mut skipped: Vec<String> = Vec::new();
    for s in picked {
        for t in targets {
            let installed_now = match install_fn(reg, s, t) {
                Ok(()) => {
                    ui::progress_or_print(
                        &pb,
                        ui::ok(format!("已安装 {s} → {t}（{}）", ui::method_label(method))),
                    );
                    true
                }
                Err(Error::Conflict(p)) => {
                    if yes {
                        ui::progress_or_print(
                            &pb,
                            ui::skip(format!("跳过已存在：{}", p.display())),
                        );
                        skipped.push(format!("{s} → {t}"));
                        false
                    } else {
                        // spinner 挂起再弹窗，避免两套终端绘制互相糊屏
                        let prompt = format!("{} 已存在，覆盖？", p.display());
                        let overwrite = match pb
                            .suspend(|| inquire::Confirm::new(&prompt).with_default(false).prompt())
                        {
                            Ok(v) => v,
                            // 覆盖确认的 ESC/Ctrl+C 视为不覆盖（跳过该条），不中断批次
                            Err(
                                inquire::error::InquireError::OperationCanceled
                                | inquire::error::InquireError::OperationInterrupted,
                            ) => false,
                            Err(e) => {
                                pb.finish_and_clear();
                                return Err(Error::Msg(e.to_string()));
                            }
                        };
                        if overwrite {
                            let rec = install::to_rec(t);
                            let _ = remove::remove_install(layout, cfg, reg, s, &rec);
                            let _ = remove::force_clean_conflict_path(&p);
                            if let Err(e) = install_fn(reg, s, t) {
                                pb.finish_and_clear();
                                return Err(e);
                            }
                            ui::progress_or_print(&pb, ui::ok(format!("已覆盖安装 {s} → {t}")));
                            true
                        } else {
                            skipped.push(format!("{s} → {t}"));
                            false
                        }
                    }
                }
                Err(e) => {
                    pb.finish_and_clear();
                    return Err(e);
                }
            };
            if installed_now {
                installed += 1;
                // 逐条落盘（tmp+rename 原子写，代价低）：之后任一失败提前返回时，
                // registry 都与磁盘已写入的副本保持一致，不留孤儿副本。
                if let Err(e) = reg.save(layout) {
                    pb.finish_and_clear();
                    return Err(e);
                }
            }
        }
    }
    pb.finish_and_clear();
    println!(
        "共安装 {installed}/{total} 条（{} 个技能 × {} 个目标，{}）",
        picked.len(),
        targets.len(),
        ui::method_label(method)
    );
    if !skipped.is_empty() {
        println!("跳过 {} 条：", skipped.len());
        for s in &skipped {
            println!("  {}", ui::skip(s));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_skill_picks_star_and_flag() {
        let all = vec!["a".to_string(), "b".to_string()];
        assert_eq!(resolve_skill_flag(&["*".into()], &all).unwrap(), all);
        assert_eq!(
            resolve_skill_flag(&["a".into()], &all).unwrap(),
            vec!["a".to_string()]
        );
        assert!(resolve_skill_flag(&["nope".into()], &all).is_err());
    }

    #[test]
    fn yes_defaults_skills_all_agents_detected_or_universal() {
        // -y 无 -a：检测到的；检测不到 → ["agents"]
        assert_eq!(yes_agents(&["codex"]), vec!["codex".to_string()]);
        assert_eq!(yes_agents(&[]), vec!["agents".to_string()]);
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
