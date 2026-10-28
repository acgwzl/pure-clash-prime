//! 设置操作：更新检查、Geo 数据更新、主题/语言、进程匹配、自定义监听。
//!
//! 重建：源码在 rewind 快照中丢失，方法实现取自上游 app/mod.rs 的同名方法，
//! 随后按本仓库的字段与调用点修正。

#![allow(unused_imports, dead_code)]

use super::*;
use crate::config::CustomListener;

impl PureClash {
    /// 后台自启等用户打开窗口后再检查；失败也算已尝试，避免反复请求。
    pub(super) fn check_startup_updates(&mut self, cx: &mut Context<Self>) {
        if self.startup_update_checked {
            return;
        }
        self.startup_update_checked = true;
        self.check_for_updates(true, cx);
    }

    /// 关于页检查更新：查询仓库最新发布版本并与当前版本比较。
    ///
    /// 首次打开窗口静默检查，也可通过按钮手动重查；不做自动安装，
    /// 发现新版本时在侧栏和关于页提示，并引导用户前往仓库。
    pub(super) fn check_for_updates(&mut self, silent: bool, cx: &mut Context<Self>) {
        if self.update_checking {
            return;
        }
        self.update_checking = true;
        // 重查期间保留已知更新，网络失败不能让侧栏标志消失。
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { crate::app::about::latest_release_version() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.update_checking = false;
                if let Ok(latest) = &result {
                    this.update_available = latest.as_ref().is_some_and(|latest| {
                        crate::app::about::is_newer_version(latest, Self::CURRENT_VERSION)
                    });
                }
                if !(silent && result.is_err()) {
                    this.update_status_is_release = result.is_ok() && this.update_available;
                }
                this.update_status = Some(match result {
                    Ok(Some(latest)) => {
                        if this.update_available {
                            log_info!(
                                "update",
                                "发现新版本 {latest}（当前 v{}）",
                                Self::CURRENT_VERSION
                            );
                            SharedString::from(
                                t!("about.new_version", version = latest).into_owned(),
                            )
                        } else {
                            tr("about.up_to_date")
                        }
                    }
                    Ok(None) => tr("about.no_release"),
                    Err(_) if silent => {
                        cx.notify();
                        return;
                    }
                    Err(error) => {
                        log_warn!("update", "检查应用更新失败：{error:#}");
                        SharedString::from(
                            t!(
                                "about.check_failed",
                                error = concise_error(&format!("{error:#}"), 160)
                            )
                            .into_owned(),
                        )
                    }
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// 设置页显式更新三份 Geo 数据；下载与哈希计算全部放到后台线程。
    ///
    /// 更新模块先固定官方 release 分支提交，再完整下载并原子提交同一快照。
    /// 运行中的 Mihomo 会在成功切换后重启，确保新规则库立即生效。
    pub(super) fn update_geodata(&mut self, cx: &mut Context<Self>) {
        if self.geodata_updating {
            return;
        }
        self.geodata_updating = true;
        self.geodata_status = None;
        let paths = self.paths.clone();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { crate::mihomo::geodata::update_from_official(&paths) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.geodata_updating = false;
                match result {
                    Ok(UpdateOutcome::UpToDate(info)) => {
                        this.geodata_info = info;
                        this.geodata_status = Some(tr("settings.geodata_up_to_date"));
                    }
                    Ok(UpdateOutcome::Updated(info)) => {
                        let revision = short_revision(&info.revision);
                        let restart_required = this.core_active();
                        this.geodata_info = info;
                        log_info!(
                            "geodata",
                            "Geo 数据已更新到 {revision}{}",
                            if restart_required {
                                "，重启内核生效"
                            } else {
                                ""
                            }
                        );
                        this.geodata_status = Some(SharedString::from(if restart_required {
                            t!("settings.geodata_updated_reloaded", revision = revision)
                                .into_owned()
                        } else {
                            t!("settings.geodata_updated", revision = revision).into_owned()
                        }));
                        if restart_required {
                            this.restart_core(cx);
                        }
                    }
                    Err(error) => {
                        log_error!("geodata", "更新 Geo 数据失败：{error:#}");
                        this.geodata_status = Some(SharedString::from(
                            t!(
                                "settings.geodata_update_failed",
                                error = concise_error(&format!("{error:#}"), 180)
                            )
                            .into_owned(),
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 标题栏和设置页共用同一主题状态，确保两个入口始终同步。
    pub(super) fn toggle_theme(&mut self, cx: &mut Context<Self>) {
        let previous = self.config.theme;
        self.config.theme = previous.toggled();

        if let Err(error) = self.config.save(&self.paths.config_file) {
            // 写入失败时恢复旧值，避免界面状态与磁盘配置不一致。
            self.config.theme = previous;
            log_warn!("app", "保存主题配置失败：{error:#}");
            return;
        }

        cx.notify();
    }

    /// 语言切换立即更新全局 locale 和主配置，保存失败时完整回滚。
    pub(super) fn toggle_language(&mut self, cx: &mut Context<Self>) {
        self.set_language(self.config.language.toggled(), cx);
    }

    pub(super) fn set_language(&mut self, language: Language, cx: &mut Context<Self>) {
        if language == self.config.language {
            return;
        }

        let previous = self.config.language;
        self.config.language = language;
        rust_i18n::set_locale(language.code());

        if let Err(error) = self.config.save(&self.paths.config_file) {
            self.config.language = previous;
            rust_i18n::set_locale(previous.code());
            log_warn!("app", "保存语言配置失败：{error:#}");
            return;
        }

        // 更新结果文案绑定生成时的 locale，切换语言后清空以避免混合显示。
        self.geodata_status = None;
        // 输入框占位文案在实体创建时固定，语言切换后按新 locale 重建。
        self.profile_form_name = cx.new(|cx| TextInput::new(t!("profiles.name_placeholder"), cx));
        self.profile_form_url = cx.new(|cx| TextInput::new(t!("profiles.url_placeholder"), cx));
        cx.notify();
    }

    /// 强制进程名开关：app.json 先行持久化，基线与 runtime 走同一 `-t` 事务；
    /// 运行中的内核经 controller 热切换，不重启进程、不断开存量连接。
    pub(super) fn toggle_find_process(&mut self, cx: &mut Context<Self>) {
        let Some(previous_baseline) = self.baseline.clone() else {
            self.integration_error = Some(t!("app.baseline_missing").into_owned());
            cx.notify();
            return;
        };
        let enabled = !self.config.find_process_always;

        // app.json 是开关的唯一事实来源；保存失败时不产生任何后续变更。
        let previous_setting = self.config.find_process_always;
        self.config.find_process_always = enabled;
        if let Err(error) = self.config.save(&self.paths.config_file) {
            self.config.find_process_always = previous_setting;
            log_warn!("app", "保存强制进程名开关失败：{error:#}");
            self.integration_error = Some(concise_error(&format!("{error:#}"), 160));
            cx.notify();
            return;
        }

        let mut desired = previous_baseline;
        desired.find_process_always = enabled;
        match self.apply_find_process_configuration(&desired, cx) {
            Ok(()) => {
                log_info!(
                    "app",
                    "强制进程名已{}",
                    if enabled { "开启" } else { "关闭" }
                );
                self.integration_error = None;
            }
            Err(error) => {
                // 配置事务失败时回滚 app.json，保持开关显示与实际生效一致。
                self.config.find_process_always = previous_setting;
                let _ = self.config.save(&self.paths.config_file);
                log_error!("app", "应用强制进程名配置失败：{error:#}");
                self.integration_error = Some(concise_error(&format!("{error:#}"), 180));
            }
        }
        cx.notify();
    }

    /// 提交进程匹配配置：先 `-t` 终审再原子落盘基线与 runtime，任一步失败
    /// 都不改变磁盘状态；成功后运行中的内核经 controller 即时生效。
    pub(super) fn apply_find_process_configuration(
        &mut self,
        desired: &crate::mihomo::config::LocalBaseline,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let runtime = profile::prepare_runtime(
            &self.paths,
            Some(desired),
            &self.config.mihomo_version,
            self.active_profile.as_deref(),
        )?;
        // 切换前快照：运行中内核 PATCH 失败时按此整体回滚（内存开关、
        // app.json、基线与 runtime），保证界面显示与实际运行状态一致。
        // 调用方 toggle 以取反方式切换，旧开关值即 !desired 的值；
        // 能走到热切换说明基线存在（controller 由基线构造）。
        let rollback_setting = !desired.find_process_always;
        let Some(rollback_baseline) = self.baseline.clone() else {
            return Ok(());
        };
        let rollback_runtime = std::fs::read_to_string(&self.paths.runtime_mihomo_config_file).ok();
        save_baseline(&self.paths, desired)?;
        if let Err(error) = write_runtime(&self.paths, &runtime) {
            // runtime 写入失败时回滚基线，保持磁盘一致。
            if let Some(previous) = self.baseline.as_ref() {
                let _ = save_baseline(&self.paths, previous);
            }
            return Err(error);
        }
        self.baseline = Some(desired.clone());
        // 内核运行中热切换；正在启动的内核沿用旧模式，下次重启从磁盘生效。
        if self.mihomo_running()
            && let Some(controller) = self.controller()
        {
            let mode = if desired.find_process_always {
                "always"
            } else {
                "strict"
            };
            cx.spawn(async move |this, cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move { controller.patch_find_process_mode(mode) })
                    .await;
                if let Err(error) = result {
                    let _ = this.update(cx, |this, cx| {
                        log_warn!(
                            "app",
                            "运行中内核切换进程匹配模式失败，回滚开关设置：{error:#}"
                        );
                        // 完整回滚四处状态，与磁盘写入事务的失败回滚对齐：
                        // 内存开关、app.json、local.yaml、runtime.yaml。
                        this.config.find_process_always = rollback_setting;
                        if let Err(save_error) = this.config.save(&this.paths.config_file) {
                            log_warn!("app", "回滚 app.json 失败：{save_error:#}");
                        }
                        if let Err(baseline_error) = save_baseline(&this.paths, &rollback_baseline)
                        {
                            log_warn!("app", "回滚本地基线失败：{baseline_error:#}");
                        }
                        if let Some(old_runtime) = rollback_runtime.as_deref()
                            && let Err(runtime_error) = write_runtime(&this.paths, old_runtime)
                        {
                            log_warn!("app", "回滚运行时配置失败：{runtime_error:#}");
                        }
                        this.baseline = Some(rollback_baseline);
                        this.integration_error = Some(
                            t!(
                                "app.find_process_rollback",
                                error = concise_error(&format!("{error:#}"), 160)
                            )
                            .into_owned(),
                        );
                        cx.notify();
                    });
                }
            })
            .detach();
        }
        Ok(())
    }







fn info_line(
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
    positive: bool,
    palette: Palette,
) -> AnyElement {
    let label = label.into();
    let value = value.into();
    div()
        .min_h(px(34.0))
        .flex()
        .items_center()
        .justify_between()
        .border_b_1()
        .border_color(palette.border)
        .child(div().text_xs().text_color(palette.muted).child(label))
        .child(
            div()
                .text_xs()
                .text_color(if positive {
                    palette.success
                } else {
                    palette.text
                })
                .child(value),
        )
        .into_any_element()
}
}

impl PureClash {
    /// 切换配置文件页的展示方式（紧凑列表 / 详细卡片）。
    pub(super) fn toggle_profiles_view(&mut self, cx: &mut Context<Self>) {
        self.profiles_view = match self.profiles_view {
            ProfilesView::List => ProfilesView::Grid,
            ProfilesView::Grid => ProfilesView::List,
        };
        cx.notify();
    }

    /// 拉取内核规则快照；规则页的命中统计与连接页共用同一份数据。
    pub(super) fn fetch_rules(&mut self, cx: &mut Context<Self>) {
        if self.rules_loading || !self.mihomo_running() {
            return;
        }
        let Some(controller) = self.controller() else {
            return;
        };
        self.rules_loading = true;
        self.rules_error = None;
        cx.notify();
        let generation = self.core_start_generation;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { controller.rules() })
                .await;
            let _ = this.update(cx, |this, cx| {
                if generation != this.core_start_generation || !this.mihomo_running() {
                    return;
                }
                this.rules_loading = false;
                match result {
                    Ok(snapshot) => {
                        this.rules = snapshot.rules;
                        this.rules_failing = false;
                        this.rules_shown = crate::app::rules::RULES_PAGE_SIZE;
                    }
                    Err(error) => {
                        if !this.rules_failing {
                            log_error!("rules", "拉取规则失败：{error:#}");
                        }
                        this.rules_failing = true;
                        this.rules_error = Some(concise_error(&format!("{error:#}"), 160));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 切换「把节点链接列表自动转换为 Clash 配置」开关。
    pub(super) fn toggle_convert_subscription(&mut self, cx: &mut Context<Self>) {
        self.config.convert_base64_subscription = !self.config.convert_base64_subscription;
        if let Err(error) = self.config.save(&self.paths.config_file) {
            log_error!("settings", "保存订阅转换开关失败：{error:#}");
            self.integration_error = Some(concise_error(&format!("{error:#}"), 160));
        }
        cx.notify();
    }
}
