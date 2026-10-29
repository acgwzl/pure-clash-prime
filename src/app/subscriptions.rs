//! 订阅管理：增删改查、导入本地配置、激活、自动更新调度、运行时应用。
//!
//! 重建：源码在 rewind 快照中丢失，方法实现取自上游 app/mod.rs 的同名方法，
//! 随后按本仓库的字段与调用点修正。

#![allow(unused_imports, dead_code)]

use super::*;

impl PureClash {
    /// 保存 profiles 列表到主配置；失败时回滚。
    pub(super) fn save_profiles(&mut self) {
        let previous = self.config.profiles.clone();
        let previous_active = self.config.active_profile.clone();
        self.config.profiles = self.profiles.clone();
        self.config.active_profile = self.active_profile.clone();
        if let Err(error) = self.config.save(&self.paths.config_file) {
            self.config.profiles = previous;
            self.config.active_profile = previous_active;
            log_error!("profile", "保存配置列表失败：{error:#}");
        }
    }

    /// 先原子保存候选元数据，再替换界面状态；编辑和排序落盘失败时保持原列表。
    pub(super) fn commit_profile_metadata(&mut self, profiles: Vec<ProfileMeta>) -> anyhow::Result<()> {
        let mut config = self.config.clone();
        config.profiles = profiles.clone();
        config.active_profile = self.active_profile.clone();
        config.save(&self.paths.config_file)?;
        self.config = config;
        self.profiles = profiles;
        Ok(())
    }

    /// 拖动仅调整展示顺序，不修改激活 ID、profile 文件或正在运行的内核。
    pub(super) fn reorder_profiles(&mut self, source_id: &str, target_id: &str, cx: &mut Context<Self>) {
        if self.profile_actions_locked() {
            return;
        }
        let mut profiles = self.profiles.clone();
        if !profile::reorder_profiles(&mut profiles, source_id, target_id) {
            return;
        }
        self.profile_error = None;
        if let Err(error) = self.commit_profile_metadata(profiles) {
            log_error!("profile", "保存配置排序失败：{error:#}");
            self.profile_error = Some(t!("profiles.save_failed").into_owned());
        }
        cx.notify();
    }

    /// 配置页动作互斥锁：模式/节点切换与恢复、手动忙态、行内编辑或自动更新进行中时，
    /// 暂停其他会修改配置列表 / profile 文件的操作，避免并发写与下标漂移。
    pub(super) fn profile_actions_locked(&self) -> bool {
        self.core_state == CoreState::Starting
            || self.tun_state == SwitchState::Starting
            || self.mode_switching
            || self.node_switching
            || self.proxies_loading
            || self.profile_busy.is_some()
            || self.editing_profile_index.is_some()
            || self.auto_update_in_flight.is_some()
    }

    /// 打开或关闭添加配置的内联表单。
    pub(super) fn toggle_profile_form(&mut self, cx: &mut Context<Self>) {
        if self.profile_actions_locked() {
            return;
        }
        self.profile_form_open = !self.profile_form_open;
        if !self.profile_form_open {
            self.profile_error = None;
        }
        cx.notify();
    }

    /// 确认添加订阅：后台下载并校验，通过后保存并自动激活。
    pub(super) fn add_subscription(&mut self, cx: &mut Context<Self>) {
        if self.profile_actions_locked() {
            return;
        }
        let url = self.profile_form_url.read(cx).content().trim().to_owned();
        if let Err(error) = profile::validate_subscription_url(&url) {
            self.profile_error = Some(error.to_string());
            cx.notify();
            return;
        }
        // 间隔先于下载校验：非法输入立即反馈，不浪费一次下载。
        let interval_input = self
            .profile_form_interval
            .read(cx)
            .content()
            .trim()
            .to_owned();
        let interval = match profile::parse_update_interval(&interval_input) {
            Ok(minutes) => minutes,
            Err(error) => {
                self.profile_error =
                    Some(t!("profiles.interval_invalid", error = error.to_string()).into_owned());
                cx.notify();
                return;
            }
        };
        let name = {
            let input = self.profile_form_name.read(cx).content().trim().to_owned();
            if input.is_empty() {
                profile::default_name_from_url(&url)
                    .unwrap_or_else(|| t!("profiles.default_name").into_owned())
            } else {
                input
            }
        };

        let version = self.config.mihomo_version.clone();
        let paths = self.paths.clone();
        self.profile_busy = Some(t!("profiles.busy_downloading").into_owned());
        let download_proxy: Option<String> = None;
        let convert = self.config.convert_base64_subscription;
        self.profile_error = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let url_inner = url.clone();
            let host = host_of_url(&url).to_owned();
            let result = cx
                .background_executor()
                .spawn(async move {
                    // 新增订阅走默认 UA；需要覆盖时可在编辑弹窗里补。
                    let content =
                        profile::download_subscription_with_usage(&url_inner, None)?.0;
                    let id = profile::new_profile_id();
                    let runtime = profile::validate_and_store(&paths, &version, &id, &content, convert)?;
                    Ok::<_, anyhow::Error>((id, runtime))
                })
                .await;

            let _ = this.update(cx, |this, cx| match result {
                Ok((id, runtime)) => {
                    this.profile_busy = None;
                    this.profile_form_open = false;
                    log_info!("profile", "订阅已下载并保存（{name}，{host}）");
                    let mut meta = profile::subscription_meta(name, url);
                    // 文件已按后台任务生成的 id 落盘，元数据必须使用同一 id。
                    meta.id = id;
                    meta.update_interval_minutes = interval;
                    if interval > 0 {
                        log_info!("profile", "新订阅自动更新间隔：{interval} 分钟");
                    }
                    let index = this.profiles.len();
                    this.profiles.push(meta);
                    this.save_profiles();
                    this.activate_profile_by_id(index, runtime, cx);
                }
                Err(error) => {
                    this.profile_busy = None;
                    log_error!("profile", "下载订阅失败（{host}）：{error:#}");
                    this.profile_error = Some(concise_error(&format!("{error:#}"), 200));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 选择并导入本地 Mihomo YAML；只保存校验后的内容副本，不持久化源文件路径。
    pub(super) fn import_local_profile(&mut self, cx: &mut Context<Self>) {
        if self.profile_actions_locked() {
            return;
        }
        let preferred_name = self.profile_form_name.read(cx).content().trim().to_owned();
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(tr("profiles.file_picker_import")),
        });
        self.profile_busy = Some(t!("profiles.busy_selecting").into_owned());
        self.profile_error = None;
        cx.notify();

        let version = self.config.mihomo_version.clone();
        let paths = self.paths.clone();
        let convert = self.config.convert_base64_subscription;
        cx.spawn(async move |this, cx| {
            let selected = match receiver.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Ok(None)) => {
                    // 用户取消属于正常流程，仅恢复表单可操作状态。
                    let _ = this.update(cx, |this, cx| {
                        this.profile_busy = None;
                        cx.notify();
                    });
                    return;
                }
                Ok(Err(error)) => {
                    let _ = this.update(cx, |this, cx| {
                        this.profile_busy = None;
                        this.profile_error = Some(
                            t!(
                                "profiles.import_failed",
                                error = concise_error(&format!("{error:#}"), 180)
                            )
                            .into_owned(),
                        );
                        cx.notify();
                    });
                    return;
                }
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.profile_busy = None;
                        this.profile_error = Some(
                            t!(
                                "profiles.import_failed",
                                error = concise_error(&error.to_string(), 180)
                            )
                            .into_owned(),
                        );
                        cx.notify();
                    });
                    return;
                }
            };
            let Some(path) = selected else {
                let _ = this.update(cx, |this, cx| {
                    this.profile_busy = None;
                    cx.notify();
                });
                return;
            };
            let name = if preferred_name.is_empty() {
                profile::default_name_from_path(&path)
                    .unwrap_or_else(|| t!("profiles.default_local_name").into_owned())
            } else {
                preferred_name
            };

            let _ = this.update(cx, |this, cx| {
                this.profile_busy = Some(t!("profiles.busy_importing").into_owned());
                cx.notify();
            });
            let result = cx
                .background_executor()
                .spawn(async move {
                    let content = profile::read_local_config(&path)?;
                    let id = profile::new_profile_id();
                    let runtime = profile::validate_and_store(&paths, &version, &id, &content, convert)?;
                    Ok::<_, anyhow::Error>((id, runtime))
                })
                .await;

            let _ = this.update(cx, |this, cx| match result {
                Ok((id, runtime)) => {
                    this.profile_busy = None;
                    this.profile_form_open = false;
                    log_info!("profile", "本地配置已导入并保存（{name}）");
                    let mut meta = profile::local_meta(name);
                    // 文件已按后台任务生成的 id 落盘，元数据必须使用同一 id。
                    meta.id = id;
                    let index = this.profiles.len();
                    this.profiles.push(meta);
                    this.save_profiles();
                    this.activate_profile_by_id(index, runtime, cx);
                }
                Err(error) => {
                    this.profile_busy = None;
                    log_error!("profile", "导入本地配置失败：{error:#}");
                    this.profile_error = Some(
                        t!(
                            "profiles.import_failed",
                            error = concise_error(&format!("{error:#}"), 180)
                        )
                        .into_owned(),
                    );
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 重新下载订阅内容并更新时间戳；激活中的配置同时刷新内核。
    pub(super) fn update_profile(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.profile_actions_locked() {
            return;
        }
        let Some(meta) = self.profiles.get(index) else {
            return;
        };
        let Some(url) = meta.url.clone() else {
            return;
        };
        let user_agent = meta.user_agent.clone();
        let version = self.config.mihomo_version.clone();
        let paths = self.paths.clone();
        let id = meta.id.clone();
        self.profile_busy = Some(t!("profiles.busy_updating").into_owned());
        let download_proxy: Option<String> = None;
        let convert = self.config.convert_base64_subscription;
        self.profile_error = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let id_inner = id.clone();
            let host = host_of_url(&url).to_owned();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let content =
                        profile::download_subscription_with_usage(&url, user_agent.as_deref())?.0;
                    let runtime =
                        profile::validate_and_store(&paths, &version, &id_inner, &content, convert)?;
                    Ok::<_, anyhow::Error>(runtime)
                })
                .await;

            let _ = this.update(cx, |this, cx| match result {
                Ok(runtime) => {
                    this.profile_busy = None;
                    let name = this
                        .profiles
                        .get(index)
                        .map(|meta| meta.name.clone())
                        .unwrap_or_default();
                    log_info!("profile", "订阅已更新（{name}，{host}）");
                    if let Some(meta) = this.profiles.get_mut(index) {
                        meta.updated_at = profile::now_secs();
                    }
                    this.save_profiles();
                    if this.active_profile.as_deref() == Some(id.as_str())
                        && let Err(error) = this.apply_runtime(&runtime, cx)
                    {
                        log_error!("profile", "应用更新后的运行时配置失败：{error:#}");
                        this.profile_error = Some(concise_error(&format!("{error:#}"), 200));
                    }
                    cx.notify();
                }
                Err(error) => {
                    this.profile_busy = None;
                    log_error!("profile", "更新订阅失败（{host}）：{error:#}");
                    this.profile_error = Some(concise_error(&format!("{error:#}"), 200));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 订阅自动更新调度：常驻任务每分钟一跳，按墙钟到期比较挑一个到期订阅。
    ///
    /// 挂在 PureClash 实体上（与连接轮询同模式），主窗口关闭、后台自启模式
    /// 下照常运行；睡眠/休眠唤醒后 due-time 立即成立，自动补跑错过的更新。
    /// 首跳延迟 90 秒，避开内核启动、系统代理自愈与就绪探针的窗口。
    pub(super) fn spawn_profile_update_scheduler(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            // 启动缓冲：登录自启阶段不与内核拉起、代理自愈抢时间。
            cx.background_executor()
                .timer(std::time::Duration::from_secs(90))
                .await;
            loop {
                cx.background_executor()
                    .timer(PROFILE_UPDATE_TICK_INTERVAL)
                    .await;
                // 主线程实体回调里挑到期订阅；与手动操作、进行中的自动更新
                // 互斥，内核 Starting 期间不做任何配置变更。
                let index = this
                    .update(cx, |this, _| {
                        if this.profile_actions_locked() || this.core_state == CoreState::Starting {
                            return None;
                        }
                        let now = profile::now_secs();
                        this.profiles
                            .iter()
                            .position(|meta| profile::subscription_due(meta, now))
                    })
                    .ok()
                    .flatten();
                let Some(index) = index else {
                    continue;
                };
                this.update(cx, |this, cx| this.auto_update_profile(index, cx))
                    .ok();
            }
        })
        .detach();
    }

    /// 自动更新一个到期订阅：静默执行（不占用 profile_busy），下载校验与
    /// 手动更新共用链路；失败只记日志与尝试时间，不打扰界面。
    ///
    /// 应用策略分三级：内容与磁盘一致时只刷时间戳跳过一切配置变更（也不必
    /// 再跑内核 `-t` 终审）；有变更且命中激活订阅时优先 controller 热重载
    /// （`PUT /configs?force=true`，进程存活、既有连接不断），热重载失败才
    /// 回退整进程重启，与 Clash Verge Rev 的应用姿态一致。
    ///
    /// 互斥与成功标记的时序：`auto_update_in_flight` 保持到本次更新的应用
    /// 真正结束（热重载完成或回退重启发起）才释放，期间配置页全部动作锁定，
    /// 避免热重载晚到的结果覆盖用户刚做的变更。`updated_at` 只在内容确定
    /// 持久化后刷新：写 runtime 失败会回滚 profile 文件并保留旧 updated_at，
    /// 下次到期重新下载时内容比对不再命中，自然重走完整链路修复 runtime。
    pub(super) fn auto_update_profile(&mut self, index: usize, cx: &mut Context<Self>) {
        // 选中到期项后再次检查，防止行内编辑与后台更新交错修改同一订阅。
        if self.profile_actions_locked() {
            return;
        }
        let Some(meta) = self.profiles.get(index) else {
            return;
        };
        let Some(url) = meta.url.clone() else {
            return;
        };
        let id = meta.id.clone();
        let user_agent = meta.user_agent.clone();
        self.auto_update_in_flight = Some(id.clone());
        let version = self.config.mihomo_version.clone();
        let paths = self.paths.clone();
        let host = host_of_url(&url).to_owned();
        let name = meta.name.clone();
        log_info!("profile", "订阅到期，自动更新开始（{name}，{host}）");
        let download_proxy: Option<String> = None;
        let convert = self.config.convert_base64_subscription;

        cx.spawn(async move |this, cx| {
            let id_inner = id.clone();
            // Ok(None) = 内容无变化；Ok(Some((合并产物, 旧 profile 内容))) =
            // 已落盘新内容，旧内容供写 runtime 失败时回滚（None = 原本无文件）。
            let result = cx
                .background_executor()
                .spawn(async move {
                    let content =
                        profile::download_subscription_with_usage(&url, user_agent.as_deref())?.0;
                    let profile_path = profile::profile_yaml_path(&paths, &id_inner);
                    // 与磁盘上的现有内容直接比对（本地比较，语义等同哈希且
                    // 无碰撞）：一致则磁盘内容已通过校验，跳过落盘与 -t 终审。
                    let existing = std::fs::read_to_string(&profile_path).ok();
                    if existing.as_deref() == Some(content.as_str()) {
                        return Ok::<_, anyhow::Error>(None);
                    }
                    let runtime =
                        profile::validate_and_store(&paths, &version, &id_inner, &content, convert)?;
                    Ok(Some((runtime, existing)))
                })
                .await;

            let _ = this.update(cx, |this, cx| {
                // 配置可能在下载期间被删除；按 id 而非下标定位。
                let index = this.profiles.iter().position(|meta| meta.id == id);
                let Some(index) = index else {
                    this.auto_update_in_flight = None;
                    return;
                };
                // 无论后续应用结果如何都记录尝试时间，顺延下一个完整间隔。
                let now = profile::now_secs();
                if let Some(meta) = this.profiles.get_mut(index) {
                    meta.last_auto_attempt_at = now;
                }
                match result {
                    Ok(None) => {
                        // 内容无变化：只刷时间戳，不产生任何配置变更。
                        if let Some(meta) = this.profiles.get_mut(index) {
                            meta.updated_at = now;
                        }
                        this.save_profiles();
                        this.auto_update_in_flight = None;
                        log_info!(
                            "profile",
                            "订阅自动更新：内容无变化，跳过应用（{name}，{host}）"
                        );
                    }
                    Ok(Some((runtime, existing))) => {
                        // 非激活订阅只落盘 profile 文件，不动内核：落盘完成
                        // 即本次更新结束。
                        if this.active_profile.as_deref() != Some(id.as_str()) {
                            if let Some(meta) = this.profiles.get_mut(index) {
                                meta.updated_at = now;
                            }
                            this.save_profiles();
                            this.auto_update_in_flight = None;
                            log_info!(
                                "profile",
                                "订阅自动更新成功（{name}，{host}，非激活仅落盘）"
                            );
                            cx.notify();
                            return;
                        }
                        // 激活订阅：写 runtime 失败必须回滚 profile 文件并保留
                        // 旧 updated_at——否则下次到期会因下载内容与磁盘相同
                        // 走跳过路径，旧 runtime 永远得不到修复。
                        if let Err(error) = write_runtime(&this.paths, &runtime) {
                            let profile_path = profile::profile_yaml_path(&this.paths, &id);
                            let rollback = match existing.as_deref() {
                                Some(old_content) => crate::platform::file::atomic_write(
                                    &profile_path,
                                    old_content.as_bytes(),
                                ),
                                None => {
                                    std::fs::remove_file(&profile_path).map_err(anyhow::Error::from)
                                }
                            };
                            if let Err(rollback_error) = rollback {
                                // 回滚失败只记日志：该场景需要磁盘写连续两次
                                // 失败，概率极低，下次到期仍会重试。
                                log_error!("profile", "回滚订阅文件失败：{rollback_error:#}");
                            }
                            this.save_profiles();
                            this.auto_update_in_flight = None;
                            log_error!(
                                "profile",
                                "写入运行时配置失败，已回滚订阅内容（{name}，{host}）：{error:#}"
                            );
                            cx.notify();
                            return;
                        }
                        // runtime 已持久化：从这一刻起内容终将生效（热重载/
                        // 重启/下次启动），刷新 updated_at 是安全的。
                        if let Some(meta) = this.profiles.get_mut(index) {
                            meta.updated_at = now;
                        }
                        this.save_profiles();
                        log_info!("profile", "订阅自动更新成功（{name}，{host}）");
                        if !this.mihomo_running() {
                            // 内核未运行（含启动中）：新 runtime 下次启动时生效。
                            this.auto_update_in_flight = None;
                            log_info!("profile", "内核未运行，新配置将在下次启动时生效");
                            cx.notify();
                            return;
                        }
                        match this.controller() {
                            Some(controller) => {
                                // 热重载优先：互斥保持到热重载结束（或回退
                                // 重启发起）后才释放；此时 runtime 已落盘，
                                // 重启路径由 Starting 状态与启动代次机制接管。
                                let runtime_yaml = runtime;
                                cx.spawn(async move |this, cx| {
                                    let result = cx
                                        .background_executor()
                                        .spawn(
                                            async move { controller.reload_config(&runtime_yaml) },
                                        )
                                        .await;
                                    let _ = this.update(cx, |this, cx| {
                                        this.auto_update_in_flight = None;
                                        match result {
                                            Ok(()) => {
                                                log_info!(
                                                    "profile",
                                                    "新配置已热重载生效（内核未重启）"
                                                );
                                                this.fetch_runtime_state(cx);
                                            }
                                            Err(error) => {
                                                log_warn!(
                                                    "profile",
                                                    "热重载失败，回退重启内核生效：{error:#}"
                                                );
                                                this.restart_core(cx);
                                            }
                                        }
                                        cx.notify();
                                    });
                                })
                                .detach();
                            }
                            None => {
                                // 基线不可用导致没有 controller 时退回重启路径。
                                log_warn!("profile", "controller 不可用，回退重启内核生效");
                                this.restart_core(cx);
                                this.auto_update_in_flight = None;
                            }
                        }
                    }
                    Err(error) => {
                        this.save_profiles();
                        this.auto_update_in_flight = None;
                        log_warn!("profile", "订阅自动更新失败（{name}，{host}）：{error:#}");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 打开订阅编辑器，回填链接与更新间隔；添加表单和行内编辑互斥。
    pub(super) fn edit_profile(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.profile_actions_locked() || self.profile_form_open {
            return;
        }
        let Some(meta) = self.profiles.get(index) else {
            return;
        };
        let Some(url) = meta.url.clone() else {
            return;
        };
        // 编辑器是页面级弹窗，与当前视图无关，不要顺手切换用户的视图选择。
        self.editing_profile_index = Some(index);
        let initial = if meta.update_interval_minutes == 0 {
            String::new()
        } else {
            meta.update_interval_minutes.to_string()
        };
        self.profile_edit_url
            .update(cx, |input, cx| input.set_content(url, cx));
        let ua = meta.user_agent.clone().unwrap_or_default();
        self.profile_edit_user_agent
            .update(cx, |input, cx| input.set_content(ua, cx));
        self.profile_form_interval
            .update(cx, |input, cx| input.set_content(initial, cx));
        self.profile_error = None;
        cx.notify();
    }

    /// 保存链接与间隔；全部校验及原子持久化成功后，链接改变才立即更新订阅。
    pub(super) fn save_profile_edits(&mut self, cx: &mut Context<Self>) {
        if self.profile_busy.is_some() || self.auto_update_in_flight.is_some() {
            return;
        }
        let Some(index) = self.editing_profile_index else {
            return;
        };
        let mut profiles = self.profiles.clone();
        let Some(meta) = profiles.get_mut(index) else {
            return;
        };
        let result = profile::edit_subscription_metadata(
            meta,
            self.profile_edit_url.read(cx).content(),
            self.profile_form_interval.read(cx).content(),
            self.profile_edit_user_agent.read(cx).content(),
            profile::now_secs(),
        );
        let url_changed = match result {
            Ok(changed) => changed,
            Err(error) => {
                self.profile_error =
                    Some(t!("profiles.edit_invalid", error = error.to_string()).into_owned());
                cx.notify();
                return;
            }
        };
        if let Err(error) = self.commit_profile_metadata(profiles) {
            // 保留输入和编辑态，用户可重试；不启动下载，也不显示未落盘的新值。
            log_error!("profile", "保存订阅编辑失败：{error:#}");
            self.profile_error = Some(t!("profiles.save_failed").into_owned());
            cx.notify();
            return;
        }
        self.editing_profile_index = None;
        self.profile_error = None;
        self.profile_edit_url
            .update(cx, |input, cx| input.set_content(String::new(), cx));
        if url_changed {
            // 保留新地址以便重试；下载或校验失败时原 profile/runtime 仍然可用。
            log_info!("profile", "订阅链接已修改，立即更新订阅");
            self.update_profile(index, cx);
        }
        cx.notify();
    }

    /// 取消行内编辑，不落盘；清除链接输入，避免长期保留已放弃的地址。
    pub(super) fn cancel_profile_edit(&mut self, cx: &mut Context<Self>) {
        if self.editing_profile_index.take().is_some() {
            self.profile_edit_url
                .update(cx, |input, cx| input.set_content(String::new(), cx));
            self.profile_edit_user_agent
                .update(cx, |input, cx| input.set_content(String::new(), cx));
            self.profile_error = None;
            cx.notify();
        }
    }

    /// 删除配置；激活中的配置会同时回退到内置默认配置。
    pub(super) fn delete_profile(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.profile_actions_locked() {
            return;
        }
        let Some(meta) = self.profiles.get(index) else {
            return;
        };
        let id = meta.id.clone();
        let name = meta.name.clone();
        let was_active = self.active_profile.as_deref() == Some(id.as_str());
        let previous_content = if was_active {
            match profile::read_profile(&self.paths, &id) {
                Ok(content) => Some(content),
                Err(error) => {
                    self.profile_error = Some(concise_error(&format!("{error:#}"), 200));
                    cx.notify();
                    return;
                }
            }
        } else {
            None
        };
        let default_runtime = if was_active {
            match profile::prepare_runtime(
                &self.paths,
                self.baseline.as_ref(),
                &self.config.mihomo_version,
                None,
            ) {
                Ok(runtime) => Some(runtime),
                Err(error) => {
                    self.profile_error = Some(concise_error(&format!("{error:#}"), 200));
                    cx.notify();
                    return;
                }
            }
        } else {
            None
        };
        if let Err(error) = profile::delete_profile_file(&self.paths, &id) {
            self.profile_error = Some(concise_error(&format!("{error:#}"), 200));
            cx.notify();
            return;
        }
        if let Some(runtime) = default_runtime
            && let Err(error) = write_runtime(&self.paths, &runtime)
        {
            // runtime 提交失败时恢复刚删除的配置，保持激活态和当前内核均不变。
            let restore_error = previous_content.as_deref().and_then(|content| {
                crate::platform::file::atomic_write(
                    &profile::profile_yaml_path(&self.paths, &id),
                    content.as_bytes(),
                )
                .err()
            });
            self.profile_error = Some(match restore_error {
                Some(restore_error) => concise_error(
                    &format!("{error:#}；恢复原配置也失败：{restore_error:#}"),
                    240,
                ),
                None => concise_error(&format!("{error:#}"), 200),
            });
            cx.notify();
            return;
        }
        self.profiles.remove(index);
        log_info!(
            "profile",
            "已删除配置 {name}{}",
            if was_active {
                "（激活中，已切回默认配置）"
            } else {
                ""
            }
        );
        if was_active {
            self.active_profile = None;
            if self.core_active() {
                self.restart_core(cx);
            }
        } else {
            cx.notify();
        }
        self.save_profiles();
    }

    /// 点击行激活配置：校验合并产物后写入 runtime.yaml，运行中则重启内核。
    pub(super) fn activate_profile_clicked(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.profile_actions_locked() {
            return;
        }
        let Some(meta) = self.profiles.get(index) else {
            return;
        };
        if self.active_profile.as_deref() == Some(meta.id.as_str()) {
            return;
        }
        let id = meta.id.clone();
        let name = meta.name.clone();
        let version = self.config.mihomo_version.clone();
        let paths = self.paths.clone();
        let convert = self.config.convert_base64_subscription;
        self.profile_busy = Some(t!("profiles.busy_activating").into_owned());
        self.profile_error = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let id_inner = id.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let content = profile::read_profile(&paths, &id_inner)?;
                    let runtime = merge_runtime(&content, &ensure_baseline(&paths)?)?;
                    validate_kernel_config(&paths, &version, &runtime)?;
                    Ok::<_, anyhow::Error>((content, runtime))
                })
                .await;

            let _ = this.update(cx, |this, cx| match result {
                Ok((_, runtime)) => {
                    this.profile_busy = None;
                    match this.apply_runtime(&runtime, cx) {
                        Ok(()) => {
                            log_info!("profile", "已激活配置 {name}");
                            this.active_profile = Some(id);
                            this.save_profiles();
                        }
                        Err(error) => {
                            log_error!("profile", "激活配置 {name} 时写入 runtime 失败：{error:#}");
                            this.profile_error = Some(concise_error(&format!("{error:#}"), 200));
                            cx.notify();
                        }
                    }
                }
                Err(error) => {
                    this.profile_busy = None;
                    log_error!("profile", "激活配置 {name} 前的校验失败：{error:#}");
                    this.profile_error = Some(concise_error(&format!("{error:#}"), 200));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 切回内置默认配置（仅 DIRECT）：与订阅激活同一条校验链路，成功后记录。
    pub(super) fn activate_default_profile(&mut self, cx: &mut Context<Self>) {
        if self.profile_actions_locked() || self.active_profile.is_none() {
            return;
        }
        let version = self.config.mihomo_version.clone();
        let paths = self.paths.clone();
        let convert = self.config.convert_base64_subscription;
        self.profile_busy = Some(t!("profiles.busy_activating").into_owned());
        self.profile_error = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let default_path = &paths.default_mihomo_config_file;
                    let content = std::fs::read_to_string(default_path).map_err(|error| {
                        anyhow::anyhow!("无法读取默认配置 {}: {error}", default_path.display())
                    })?;
                    let runtime = merge_runtime(&content, &ensure_baseline(&paths)?)?;
                    validate_kernel_config(&paths, &version, &runtime)?;
                    Ok::<_, anyhow::Error>(runtime)
                })
                .await;

            let _ = this.update(cx, |this, cx| match result {
                Ok(runtime) => {
                    this.profile_busy = None;
                    match this.apply_runtime(&runtime, cx) {
                        Ok(()) => {
                            log_info!("profile", "已切回内置默认配置");
                            // active_profile 记为空即代表默认配置，下次启动同样生效。
                            this.active_profile = None;
                            this.save_profiles();
                        }
                        Err(error) => {
                            log_error!("profile", "切回默认配置时写入 runtime 失败：{error:#}");
                            this.profile_error = Some(concise_error(&format!("{error:#}"), 200));
                            cx.notify();
                        }
                    }
                }
                Err(error) => {
                    this.profile_busy = None;
                    log_error!("profile", "切回默认配置前的校验失败：{error:#}");
                    this.profile_error = Some(concise_error(&format!("{error:#}"), 200));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 激活/更新配置后的落地动作：运行中则重启内核让配置真实生效。
    pub(super) fn apply_runtime(&mut self, runtime_yaml: &str, cx: &mut Context<Self>) -> anyhow::Result<()> {
        // 先落地 runtime.yaml，未运行时下次启动直接使用新配置。
        write_runtime(&self.paths, runtime_yaml)?;
        if self.core_active() {
            self.restart_core(cx);
        } else {
            cx.notify();
        }
        Ok(())
    }

    /// 添加订阅后按索引激活：补齐激活态并重启内核。
    pub(super) fn activate_profile_by_id(
        &mut self,
        index: usize,
        runtime_yaml: String,
        cx: &mut Context<Self>,
    ) {
        let id = self.profiles.get(index).map(|meta| meta.id.clone());
        if let Some(id) = id {
            match self.apply_runtime(&runtime_yaml, cx) {
                Ok(()) => {
                    log_info!("profile", "新配置已激活并按需重启内核");
                    self.active_profile = Some(id);
                    self.save_profiles();
                }
                Err(error) => {
                    log_error!("profile", "应用新配置的运行时写入失败：{error:#}");
                    self.profile_error = Some(concise_error(&format!("{error:#}"), 200));
                    cx.notify();
                }
            }
        }
    }
}

impl PureClash {
    /// 打开某个订阅的操作菜单（右键卡片触发）。
    pub(super) fn open_profile_menu(&mut self, id: String, cx: &mut Context<Self>) {
        self.profile_menu_for = Some(id);
        cx.notify();
    }

    /// 关闭订阅操作菜单。
    pub(super) fn close_profile_menu(&mut self, cx: &mut Context<Self>) {
        self.profile_menu_for = None;
        cx.notify();
    }
}

impl PureClash {
    /// 串行更新全部远程订阅：逐个下载、校验、落盘，最后统一保存并应用。
    ///
    /// 串行而非并发：每次更新都要走内核 `-t` 校验与原子落盘，
    /// 并发会互相覆盖 runtime，也会让进度提示失去意义。
    pub(super) fn update_all_profiles(&mut self, cx: &mut Context<Self>) {
        if self.profile_actions_locked() {
            return;
        }
        // 带上名字：失败时要能告诉用户是哪一条没更新成功。
        let targets: Vec<(usize, String, String, Option<String>, String)> = self
            .profiles
            .iter()
            .enumerate()
            .filter_map(|(index, meta)| {
                meta.url
                    .clone()
                    .map(|url| (index, url, meta.id.clone(), meta.user_agent.clone(), meta.name.clone()))
            })
            .collect();
        if targets.is_empty() {
            return;
        }
        let total = targets.len();
        let version = self.config.mihomo_version.clone();
        let paths = self.paths.clone();
        let convert = self.config.convert_base64_subscription;
        let active_id = self.active_profile.clone();
        self.profile_busy = Some(
            t!(
                "profiles.busy_updating_all",
                done = "0",
                total = total.to_string()
            )
            .into_owned(),
        );
        self.profile_error = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            // 阶段一：并发下载。订阅站点的响应速度差异极大，串行时一个慢站
            // 会把整批拖到超时；下载是纯 IO，并发不会互相干扰。
            let (tx, rx) = async_channel::bounded(total);
            for (index, url, id, user_agent, name) in targets {
                let tx = tx.clone();
                cx.background_executor()
                    .spawn(async move {
                        let result = (|| -> Result<(usize, String, String, String), (String, anyhow::Error)> {
                            match profile::download_subscription_with_usage(&url, user_agent.as_deref()) {
                                Ok((content, _)) => Ok((index, id, content, name)),
                                Err(error) => Err((name, error)),
                            }
                        })();
                        let _ = tx.send(result).await;
                    })
                    .detach();
            }
            drop(tx);

            let mut downloaded: Vec<(usize, String, String, String)> = Vec::new();
            let mut download_failed_names: Vec<String> = Vec::new();
            let mut download_done = 0usize;
            while let Ok(result) = rx.recv().await {
                download_done += 1;
                match result {
                    Ok(item) => downloaded.push(item),
                    Err((name, error)) => {
                        log_warn!("profile", "批量更新下载失败（{name}）：{error:#}");
                        download_failed_names.push(name);
                    }
                }
                // 下载阶段也推进进度：否则界面上会长时间停在 0。
                let done_text = download_done.to_string();
                let total_text = total.to_string();
                let _ = this.update(cx, |this, cx| {
                    this.profile_busy = Some(
                        t!(
                            "profiles.busy_downloading_all",
                            done = done_text,
                            total = total_text
                        )
                        .into_owned(),
                    );
                    cx.notify();
                });
            }

            // 阶段二：串行校验落盘。`-t` 校验与原子写是重活，串行避免同时
            // 拉起多个内核进程；到这里已经没有网络等待，速度由本地决定。
            let mut ok = 0usize;
            let mut failed = 0usize;
            let mut active_runtime: Option<(String, String)> = None;
            let downloaded_total = downloaded.len();
            let download_failed = total.saturating_sub(downloaded_total);
            failed += download_failed;

            let mut failed_names = download_failed_names;
            for (step, (index, id, content, name)) in downloaded.into_iter().enumerate() {
                let _ = this.update(cx, |this, cx| {
                    this.profile_busy = Some(
                        t!(
                            "profiles.busy_updating_all",
                            done = (step + 1 + download_failed).to_string(),
                            total = total.to_string()
                        )
                        .into_owned(),
                    );
                    cx.notify();
                });

                let paths_inner = paths.clone();
                let version_inner = version.clone();
                let id_inner = id.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        profile::validate_and_store(&paths_inner, &version_inner, &id_inner, &content, convert)
                    })
                    .await;

                match result {
                    Ok(runtime) => {
                        ok += 1;
                        let _ = this.update(cx, |this, _| {
                            if let Some(meta) = this.profiles.get_mut(index) {
                                meta.updated_at = profile::now_secs();
                            }
                        });
                        if active_id.as_deref() == Some(id.as_str()) {
                            active_runtime = Some((id, runtime));
                        }
                    }
                    Err(error) => {
                        failed += 1;
                        failed_names.push(name.clone());
                        log_warn!("profile", "批量更新落盘失败（{name}）：{error:#}");
                    }
                }
            }

            let _ = this.update(cx, |this, cx| {
                this.profile_busy = None;
                this.save_profiles();
                if let Some((id, runtime)) = active_runtime {
                    if this.active_profile.as_deref() == Some(id.as_str()) {
                        if let Err(error) = this.apply_runtime(&runtime, cx) {
                            log_error!("profile", "应用批量更新后的运行时配置失败：{error:#}");
                            this.profile_error = Some(concise_error(&format!("{error:#}"), 200));
                        }
                    }
                }
                if failed > 0 {
                    let names = if failed_names.is_empty() {
                        String::new()
                    } else {
                        format!("（{}）", failed_names.join("、"))
                    };
                    this.profile_error = Some(
                        t!(
                            "profiles.update_all_partial",
                            ok = ok.to_string(),
                            failed = failed.to_string(),
                            names = names
                        )
                        .into_owned(),
                    );
                }
                log_info!("profile", "批量更新完成：成功 {ok}，失败 {failed}");
                cx.notify();
            });
        })
        .detach();
    }
}
