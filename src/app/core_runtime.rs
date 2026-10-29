//! 内核运行时：启动/停止/重启内核、状态机迁移、controller 访问与运行时快照同步。
//!
//! 重建：源码在 rewind 快照中丢失，方法实现取自上游 app/mod.rs 的同名方法，
//! 随后按本仓库的字段与调用点修正。

#![allow(unused_imports, dead_code)]

use super::*;

impl PureClash {
    /// 内核是否处于运行状态（controller 已就绪）。
    pub(super) fn mihomo_running(&self) -> bool {
        matches!(self.core_state, CoreState::Running)
    }

    /// 内核是否允许接受启停操作。
    pub(super) fn core_operable(&self) -> bool {
        self.core_state != CoreState::Starting
    }

    /// 内核已经运行或正在异步启动；替换 runtime/Geo 数据时两种状态都必须重启。
    pub(super) fn core_active(&self) -> bool {
        self.core_state != CoreState::Stopped
    }

    pub(super) fn toggle_core(&mut self, cx: &mut Context<Self>) {
        if self.mihomo_running() {
            log_info!("core", "用户请求停止内核");
            // 系统代理指向本机内核端口，停内核前先恢复用户原有代理设置。
            self.disable_system_proxy();
            self.stop_core();
            // TUN 由内核承载，内核停止即失效；同步回退避免界面与下次启动
            // 仍显示/启用已停止的 TUN。
            if let Err(error) = self.revert_tun(cx) {
                log_error!("tun", "停止内核时回退 TUN 配置失败：{error:#}");
                self.integration_error = Some(concise_error(&format!("{error:#}"), 180));
            }
        } else {
            self.start_core(cx);
        }
        cx.notify();
    }

    /// 启动内核：Windows 在独立线程完成配置校验与 UAC，避免重入 GPUI 更新回调。
    ///
    /// `ShellExecuteExW(runas)` 会运行嵌套消息循环；若在 GPUI 实体更新回调里同步
    /// 调用，连接轮询等前台任务可能重入 `App` 并触发 `RefCell already borrowed`。
    /// Linux 的 pdeathsig 与创建进程的线程生命周期绑定，仍保持当前线程启动。
    pub(super) fn start_core(&mut self, cx: &mut Context<Self>) {
        if !self.core_operable() || self.mihomo_process.is_some() {
            return;
        }
        let reservation = if let Some(baseline) = self.baseline.as_mut() {
            match baseline.reserve_subscription_direct_port() {
                Ok(listener) => listener,
                Err(error) => {
                    self.mihomo_error = Some(
                        t!(
                            "app.core_start_failed",
                            error = concise_error(&format!("{error:#}"), 180)
                        )
                        .into_owned(),
                    );
                    cx.notify();
                    return;
                }
            }
        } else {
            None
        };
        self.subscription_direct_ready = false;
        self.core_start_generation = self.core_start_generation.wrapping_add(1);
        let generation = self.core_start_generation;
        self.core_state = CoreState::Starting;
        self.tun_state = if self.tun_configured {
            SwitchState::Starting
        } else {
            SwitchState::Off
        };
        self.mihomo_error = None;
        cx.notify();

        let paths = self.paths.clone();
        let version = self.config.mihomo_version.clone();
        let config_file = self.paths.runtime_mihomo_config_file.clone();
        // TUN 需要系统网络权限；UAC/polkit 弹窗即用户的显式授权。
        let elevated = self.tun_configured;
        let allow_interactive_elevation = self.interactive_elevation_allowed;
        log_info!(
            "core",
            "发起内核启动（版本 {}{}，runtime {}）",
            version,
            if elevated { "，提权/TUN" } else { "" },
            config_file.display()
        );
        // Mihomo 需要自行绑定端口；释放到绑定之间仍可能被抢占，启动后还要检查实际转发。
        drop(reservation);

        #[cfg(target_os = "windows")]
        {
            let (sender, receiver) = async_channel::bounded(1);
            let thread = std::thread::Builder::new()
                .name("pure-clash-kernel-start".to_owned())
                .spawn(move || {
                    let result = MihomoProcess::start(
                        &paths,
                        &version,
                        &config_file,
                        elevated,
                        allow_interactive_elevation,
                    );
                    // 应用已退出时接收端会关闭；result 的 Drop 会回收已启动的内核。
                    let _ = sender.send_blocking(result);
                });
            if let Err(error) = thread {
                self.finish_core_start(
                    generation,
                    Err(anyhow::anyhow!("无法创建内核启动线程：{error}")),
                    cx,
                );
                return;
            }
            cx.spawn(async move |this, cx| {
                let result = receiver
                    .recv()
                    .await
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("内核启动线程未返回结果")));
                let _ = this.update(cx, |this, cx| {
                    this.finish_core_start(generation, result, cx);
                });
            })
            .detach();
        }

        #[cfg(not(target_os = "windows"))]
        {
            let result = MihomoProcess::start(
                &paths,
                &version,
                &config_file,
                elevated,
                allow_interactive_elevation,
            );
            self.finish_core_start(generation, result, cx);
        }
    }

    /// 提交一次内核启动结果；过期结果会随 `MihomoProcess::Drop` 立即终止。
    pub(super) fn finish_core_start(
        &mut self,
        generation: u64,
        result: anyhow::Result<MihomoProcess>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.core_start_generation || self.core_state != CoreState::Starting {
            drop(result);
            if self.core_restart_pending && self.core_state == CoreState::Starting {
                // 等旧启动线程完整返回并回收进程后，再按最新 runtime 发起唯一一次启动。
                self.core_restart_pending = false;
                self.core_state = CoreState::Stopped;
                self.start_core(cx);
            }
            return;
        }
        match result {
            Ok(process) => {
                let pid = process.pid();
                log_info!(
                    "core",
                    "内核进程已创建（pid {}）",
                    pid.map(|pid| pid.to_string())
                        .unwrap_or_else(|| "未知".into())
                );
                self.mihomo_process = Some(process);
                self.mihomo_error = None;
                // 运行时长与峰值都按「本次内核会话」计；重启即重新计时。
                self.core_started_at = Some(std::time::Instant::now());
                self.peak_down_speed = 0;
                self.peak_up_speed = 0;
                // 换内核后重新自动检测一次出口信息。
                self.net_check_auto_done = false;
                // 旧内核会话的检测若还在路上，结果会被 generation 守卫
                // 丢弃；这里同步复位，UI 不残留「检测中」。
                self.net_check_loading = false;
                self.spawn_readiness_probe(generation, cx);
            }
            Err(error) => {
                self.mihomo_process = None;
                self.core_state = CoreState::Stopped;
                self.tun_state = SwitchState::Off;
                // 提权启动被拒绝（用户取消 UAC/polkit）或校验失败时，回退关闭 TUN
                // 并以普通权限重新拉起内核，保证用户始终有可用代理。
                if self.tun_configured {
                    log_warn!(
                        "tun",
                        "TUN 内核启动失败，回退关闭 TUN 并重启普通内核：{error:#}"
                    );
                    let fallback_message = tun_reverted_error(&error);
                    if let Err(revert_error) = self.revert_tun(cx) {
                        log_error!("tun", "回退 TUN 配置失败：{revert_error:#}");
                        self.integration_error = Some(concise_error(
                            &format!("{fallback_message}；回退配置失败：{revert_error:#}"),
                            220,
                        ));
                    } else {
                        self.integration_error = Some(fallback_message);
                        self.start_core(cx);
                    }
                    cx.notify();
                    return;
                }
                let detail = concise_error(&format!("{error:#}"), 240);
                // 失败详情写入日志前由日志层统一脱敏（URL 只留 host、token 掩码），
                // 界面仍展示完整原因。
                log_error!("core", "启动 Mihomo 失败：{error:#}");
                self.mihomo_error = Some(t!("app.core_start_failed", error = detail).into_owned());
            }
        }
        cx.notify();
    }

    /// 重启内核并重新拉取状态；配置切换后的真实生效路径。
    pub(super) fn restart_core(&mut self, cx: &mut Context<Self>) {
        if self.core_state == CoreState::Starting && self.mihomo_process.is_none() {
            // UAC/校验线程无法安全强制取消；使其结果过期，并把多次重启合并为一次。
            self.core_start_generation = self.core_start_generation.wrapping_add(1);
            self.core_restart_pending = true;
            cx.notify();
            return;
        }
        log_info!("core", "重启内核以应用新配置");
        self.stop_core();
        self.start_core(cx);
        cx.notify();
    }

    /// 回收 Mihomo 子进程；托盘退出和窗口内停止按钮共用同一条路径。
    ///
    /// 注意：配置切换（restart_core）也走这里，系统代理保持托管不中断，
    /// 仅在真实停机路径（手动停止/退出/内核失联）由调用方恢复代理设置。
    pub(crate) fn stop_core(&mut self) {
        // 让仍在 UAC/校验线程中的启动结果失效；结果返回后由 Drop 自动回收。
        self.core_start_generation = self.core_start_generation.wrapping_add(1);
        self.core_restart_pending = false;
        let stop_result = self.mihomo_process.take().map(|mut process| process.stop());

        self.core_state = CoreState::Stopped;
        self.tun_state = SwitchState::Off;
        self.subscription_direct_ready = false;
        if let Some(Err(error)) = stop_result {
            let detail = concise_error(&format!("{error:#}"), 240);
            log_error!("core", "停止 Mihomo 失败：{error:#}");
            self.mihomo_error = Some(t!("app.core_stop_failed", error = detail).into_owned());
        } else {
            log_info!("core", "内核已停止");
            self.mihomo_error = None;
        }

        // 内核停止后在线数据不再有效；TUN 的回退由真实停机路径（toggle_core/
        // 启动失败回退）显式调用 revert_tun，配置切换重启不在此处处理。
        self.groups.clear();
        self.proxy_group_order = None;
        self.proxies_loading = false;
        self.connections.clear();
        self.traffic_down_speed = 0;
        self.traffic_up_speed = 0;
        self.download_total = 0;
        self.upload_total = 0;
        self.delay_testing.clear();
        self.node_delays.clear();
        self.node_delay_failures.clear();
        // 新内核从健康状态重新开始统计，不能继承上一进程的失败边沿。
        self.runtime_state_failing = false;
        self.connections_failing = false;
    }

    /// 退出前的完整清理：恢复系统代理并回收内核；托盘退出专用。
    pub(crate) fn shutdown(&mut self) {
        log_info!("app", "应用退出：恢复系统代理并停止内核");
        self.disable_system_proxy();
        self.stop_core();
    }

    /// controller 客户端；基线缺失时在线功能不可用。
    pub(super) fn controller(&self) -> Option<Controller> {
        self.baseline.as_ref().map(Controller::new)
    }

    /// 后台拉取运行模式与代理组；内核未运行时忽略。
    pub(super) fn fetch_runtime_state(&mut self, cx: &mut Context<Self>) {
        self.sync_runtime_state(true, cx);
    }

    /// 首次加载、手动刷新及配置切换重读文件；后台轮询沿用配置元数据。
    pub(super) fn sync_runtime_state(&mut self, refresh_metadata: bool, cx: &mut Context<Self>) {
        if !self.mihomo_running() || self.proxies_loading || self.node_switching {
            return;
        }
        let Some(controller) = self.controller() else {
            return;
        };
        let runtime_config = self.paths.runtime_mihomo_config_file.clone();
        let refresh_metadata = refresh_metadata || self.proxy_group_order.is_none();
        let selections = self.config.selected_nodes.clone();
        let generation = self.core_start_generation;
        self.proxies_loading = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let snapshot = cx.background_executor().spawn(async move {
                // 客户端持久化不依赖内核 cache.db，普通内核和 Linux TUN 服务均适用。
                controller.restore_proxy_selections(&selections)?;
                let config = controller.configs()?;
                let proxies = controller.proxies(config.mode)?;
                let metadata = refresh_metadata.then(|| {
                    (
                        read_proxy_group_order(&runtime_config),
                        // 节点接入信息（协议/服务器/端口）来自 runtime.yaml，供概览页展示。
                        read_node_endpoints(&runtime_config),
                    )
                });
                Ok::<_, anyhow::Error>((config, proxies, metadata))
            });
            let result = snapshot.await;
            let _ = this.update(cx, |this, cx| {
                if generation != this.core_start_generation || !this.mihomo_running() {
                    return;
                }
                this.proxies_loading = false;
                match result {
                    Ok((config, mut proxies, metadata)) => {
                        if let Some((order, endpoints)) = metadata {
                            this.proxy_group_order = Some(order);
                            this.node_endpoints = endpoints;
                        }
                        // controller 返回无序映射，分组仍按订阅定义顺序展示。
                        order_groups(
                            &mut proxies.groups,
                            this.proxy_group_order.as_deref().unwrap_or_default(),
                        );
                        this.mode = ProxyMode::from_controller(config.mode);
                        this.groups = proxies.groups;
                        // 规则快照不随代理轮询返回；内核就绪后补拉一次，
                        // 否则概览页的规则计数与规则页会一直是空态。
                        if this.rules.is_empty() && !this.rules_loading {
                            this.fetch_rules(cx);
                        }
                        // 内核就绪且拿到运行配置后自动查一次出口信息，
                        // 省得用户每次开机都手动点「检测」。
                        if !this.net_check_auto_done && !this.net_check_loading {
                            this.net_check_auto_done = true;
                            // 自动检测静默进行：结果直接更新到主页连通性区域，不弹窗打断。
                            this.check_network_ex(false, cx);
                        }
                        if this.runtime_state_failing {
                            this.runtime_state_failing = false;
                            // 只清理由本次同步恢复的错误，不覆盖用户其他操作的提示。
                            if this.proxy_error.as_deref()
                                == Some(t!("proxy.nodes_restore_failed").as_ref())
                            {
                                this.proxy_error = None;
                            }
                            log_info!("controller", "controller 运行配置请求恢复正常");
                        }
                    }
                    Err(error) => {
                        this.proxy_error = Some(t!("proxy.nodes_restore_failed").into_owned());
                        // 轮询失败只做边沿记录：从成功转入失败记一条 warn，
                        // 恢复时记一条 info，避免刷屏。
                        if !this.runtime_state_failing {
                            this.runtime_state_failing = true;
                            log_warn!("controller", "拉取 controller 状态失败：{error:#}");
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 后台轮询 controller `/version`；就绪后拉取运行模式与代理组。
    pub(super) fn spawn_readiness_probe(&mut self, generation: u64, cx: &mut Context<Self>) {
        let Some(controller) = self.controller() else {
            self.core_state = CoreState::Running;
            self.restore_system_proxy_if_configured();
            return;
        };
        cx.spawn(async move |this, cx| {
            let background = cx.background_executor().clone();
            let probe = {
                let timer = background.clone();
                background.spawn(async move {
                    for _ in 0..50 {
                        if controller.version().is_ok() {
                            return Ok(());
                        }
                        timer.timer(std::time::Duration::from_millis(100)).await;
                    }
                    Err(anyhow::anyhow!("等待 Mihomo controller 超时"))
                })
            };
            let result = probe.await;
            let _ = this.update(cx, |this, cx| {
                if generation != this.core_start_generation {
                    return;
                }
                match result {
                    Ok(()) => {
                        log_info!("core", "controller 就绪，内核进入运行状态");
                        this.core_state = CoreState::Running;
                        this.mihomo_error = None;
                        this.fetch_runtime_state(cx);
                        this.verify_tun_effective(generation, cx);
                        this.restore_system_proxy_if_configured();
                    }
                    Err(error) => {
                        // 内核未能就绪：先恢复代理设置再回收进程，避免流量断在死端口上。
                        log_error!("core", "等待 Mihomo controller 就绪超时：{error:#}");
                        this.disable_system_proxy();
                        this.stop_core();
                        // TUN 开启时最常见的失败原因是缺少系统授权或平台 TUN 能力，
                        // 自动回退到无 TUN 的配置并重新拉起内核，保证用户有可用代理。
                        let mut fallback_error = None;
                        if this.tun_configured {
                            // 回退事务会校验并原子写入关闭 TUN 的配置，再拉起普通内核。
                            match this.revert_tun(cx) {
                                Ok(()) => this.start_core(cx),
                                Err(revert_error) => {
                                    log_error!("tun", "TUN 回退配置失败：{revert_error:#}");
                                    fallback_error = Some(concise_error(
                                        &format!("TUN 回退配置失败：{revert_error:#}"),
                                        180,
                                    ));
                                }
                            }
                        }
                        let start_error = t!(
                            "app.core_start_failed",
                            error = concise_error(&format!("{error:#}"), 160)
                        )
                        .into_owned();
                        this.integration_error = Some(match fallback_error {
                            Some(fallback_error) => {
                                concise_error(&format!("{start_error}；{fallback_error}"), 240)
                            }
                            None => start_error,
                        });
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }
}
