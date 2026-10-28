//! 代理操作：模式切换、节点选择、延迟测试、连接轮询与清理、策略组排序。
//!
//! 重建：源码在 rewind 快照中丢失，方法实现取自上游 app/mod.rs 的同名方法，
//! 随后按本仓库的字段与调用点修正。

#![allow(unused_imports, dead_code)]

use super::*;

/// 代理快照轮询间隔；内核 provider 异步加载完成后需要尽快反映到界面。
const PROXIES_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

impl PureClash {
    /// 持续同步分组成员和已选节点，避免界面停留在 provider 加载前的 COMPATIBLE 快照。
    pub(super) fn spawn_proxy_poll(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(PROXIES_POLL_INTERVAL).await;
                if this
                    .update(cx, |this, cx| {
                        // 复用代理加载互斥与启动代次校验；恢复记忆节点不能与用户操作竞态。
                        if this.mihomo_running() && !this.profile_actions_locked() {
                            this.sync_runtime_state(false, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// 实时连接与流量轮询：内核运行期间每秒拉取一次 /connections 快照。
    ///
    /// 任务随应用常驻，内核停止时只跳过拉取，无需在启停路径上管理生命周期；
    /// 应用实体释放后循环自动退出。网速由相邻快照的累计字节数差分得出。
    pub(super) fn spawn_connection_poll(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(CONNECTIONS_POLL_INTERVAL)
                    .await;
                let controller = this
                    .update(cx, |this, _| {
                        this.mihomo_running().then(|| this.controller()).flatten()
                    })
                    .ok()
                    .flatten();
                let Some(controller) = controller else {
                    continue;
                };
                let snapshot = cx
                    .background_executor()
                    .spawn(async move { controller.connections() })
                    .await;
                let applied = this.update(cx, |this, cx| {
                    // 快照返回时内核可能刚被停止；保持清空状态即可。
                    if this.mihomo_running() {
                        match snapshot {
                            Ok(data) => {
                                if this.connections_failing {
                                    this.connections_failing = false;
                                    log_info!("controller", "controller 连接轮询恢复正常");
                                }
                                this.apply_connections(data);
                            }
                            Err(error) => {
                                // controller 短暂不可达时只清零流量；若受管进程也已
                                // 退出，则立即撤销所有依赖内核的真实运行状态。
                                let process_running = this
                                    .mihomo_process
                                    .as_mut()
                                    .is_some_and(MihomoProcess::is_running);
                                if process_running {
                                    if !this.connections_failing {
                                        this.connections_failing = true;
                                        log_warn!(
                                            "controller",
                                            "连接轮询失败（内核进程仍在运行）：{error:#}"
                                        );
                                    }
                                    this.clear_live_traffic();
                                } else {
                                    log_error!(
                                        "core",
                                        "受管内核进程意外退出，自动恢复系统代理并停止内核"
                                    );
                                    this.disable_system_proxy();
                                    this.stop_core();
                                    this.integration_error =
                                        Some(t!("app.core_stopped_unexpectedly").into_owned());
                                }
                            }
                        }
                    }
                    cx.notify();
                });
                if applied.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// 应用连接快照并差分出实时网速；内核重启后累计值变小，差分自然归零。
    pub(super) fn apply_connections(&mut self, snapshot: ConnectionsSnapshot) {
        self.traffic_down_speed = snapshot.download_total.saturating_sub(self.download_total);
        self.traffic_up_speed = snapshot.upload_total.saturating_sub(self.upload_total);
        self.download_total = snapshot.download_total;
        self.upload_total = snapshot.upload_total;
        self.peak_down_speed = self.peak_down_speed.max(self.traffic_down_speed);
        self.peak_up_speed = self.peak_up_speed.max(self.traffic_up_speed);
        // 内存随同一次轮询采样：OpenProcess 开销远低于一帧渲染，1 秒一次足够。
        self.app_memory = crate::platform::process_rss(std::process::id());
        self.core_memory = self
            .mihomo_process
            .as_ref()
            .and_then(|process| process.pid())
            .and_then(crate::platform::process_rss);
        self.connections = snapshot.connections;
    }

    /// controller 不可达时清空实时数据；下次成功快照会重新建立基准。
    pub(super) fn clear_live_traffic(&mut self) {
        self.traffic_down_speed = 0;
        self.traffic_up_speed = 0;
        self.core_started_at = None;
        self.core_memory = None;
        self.connections.clear();
    }

    /// 关闭单条连接：先乐观移除，失败时提示；下一拍轮询会带回真实状态。
    pub(super) fn close_connection(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(controller) = self.controller() else {
            return;
        };
        self.connections.retain(|connection| connection.id != id);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { controller.close_connection(&id) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = result {
                    log_warn!("controller", "关闭单个连接失败：{error:#}");
                    this.integration_error = Some(concise_error(&format!("{error:#}"), 160));
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 关闭全部连接。
    pub(super) fn close_all_connections(&mut self, cx: &mut Context<Self>) {
        let Some(controller) = self.controller() else {
            return;
        };
        self.connections.clear();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { controller.close_all_connections() })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = result {
                    log_warn!("controller", "关闭全部连接失败：{error:#}");
                    this.integration_error = Some(concise_error(&format!("{error:#}"), 160));
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 节点当前展示的延迟：手动测速结果优先，其次取 /proxies 自带的历史值。
    /// 返回 `Some(None)` 表示最近一次手动测速失败（超时/拒绝）。
    pub(super) fn node_delay(&self, node: &NodeSnapshot) -> Option<Option<u64>> {
        if self.delay_testing.contains(&node.name) {
            return None;
        }
        if self.node_delay_failures.contains(&node.name) {
            return Some(None);
        }
        Some(Some(
            self.node_delays.get(&node.name).copied().or(node.delay)?,
        ))
    }

    /// 对策略组内全部节点测速：结果覆盖历史缓存，未返回的节点按失败处理。
    pub(super) fn test_group_delay(&mut self, name: String, cx: &mut Context<Self>) {
        let group_key = format!("group:{name}");
        if !self.mihomo_running() || self.delay_testing.contains(&group_key) {
            return;
        }
        let Some(group) = self.groups.iter().find(|group| group.name == name) else {
            return;
        };
        let nodes: Vec<String> = group.nodes.iter().map(|node| node.name.clone()).collect();
        if nodes.is_empty() {
            return;
        }
        let Some(controller) = self.controller() else {
            return;
        };
        self.delay_testing.insert(group_key.clone());
        for node in &nodes {
            self.delay_testing.insert(node.clone());
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { controller.group_delay(&name) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.delay_testing.remove(&group_key);
                for node in &nodes {
                    this.delay_testing.remove(node);
                }
                match result {
                    Ok(delays) => {
                        for node in &nodes {
                            match delays.get(node) {
                                Some(delay) => {
                                    this.node_delays.insert(node.clone(), *delay);
                                    this.node_delay_failures.remove(node);
                                }
                                // 本轮未返回即失败；失败状态让节点显示超时而非旧值。
                                None => {
                                    this.node_delays.remove(node);
                                    this.node_delay_failures.insert(node.clone());
                                }
                            }
                        }
                    }
                    Err(error) => {
                        log_debug!("proxy", "分组延迟测试失败：{error:#}");
                        this.integration_error = Some(concise_error(&format!("{error:#}"), 160));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 对单个节点测速。
    pub(super) fn test_node_delay(&mut self, name: String, cx: &mut Context<Self>) {
        if !self.mihomo_running() || self.delay_testing.contains(&name) {
            return;
        }
        let Some(controller) = self.controller() else {
            return;
        };
        self.delay_testing.insert(name.clone());
        let node_name = name.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { controller.proxy_delay(&name) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.delay_testing.remove(&node_name);
                match result {
                    Ok(delay) => {
                        this.node_delays.insert(node_name.clone(), delay);
                        this.node_delay_failures.remove(&node_name);
                    }
                    Err(_) => {
                        this.node_delays.remove(&node_name);
                        this.node_delay_failures.insert(node_name);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

/// 系统代理 / TUN 操作失败的提示横幅。
fn order_groups(groups: &mut [GroupSnapshot], order: &[String]) {
    groups.sort_by_key(|group| {
        if group.name == "GLOBAL" {
            (0usize, 0usize)
        } else {
            let position = order
                .iter()
                .position(|name| name == &group.name)
                .unwrap_or(usize::MAX);
            (1usize, position)
        }
    });
}

    /// 保存模式到基线和运行时配置；写入失败保留原基线，供用户重试。
    pub(super) fn save_mode_configuration(&mut self, mode: Mode) -> anyhow::Result<()> {
        let previous = self
            .baseline
            .clone()
            .ok_or_else(|| anyhow::anyhow!("本地基线不可用"))?;
        let mut desired = previous.clone();
        desired.mode = mode;
        // 沿用配置合并和同版本内核 -t 校验，保证重启与订阅更新行为一致。
        let runtime = profile::prepare_runtime(
            &self.paths,
            Some(&desired),
            &self.config.mihomo_version,
            self.active_profile.as_deref(),
        )?;
        save_baseline(&self.paths, &desired)?;
        if let Err(error) = write_runtime(&self.paths, &runtime) {
            return match save_baseline(&self.paths, &previous) {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(anyhow::anyhow!(
                    "{error:#}；恢复原本地基线也失败：{rollback_error:#}"
                )),
            };
        }
        self.baseline = Some(desired);
        Ok(())
    }

    /// 先热切换内核，成功后记住选择；失败不覆盖上次保存的模式。
    pub(super) fn set_mode(&mut self, mode: ProxyMode, cx: &mut Context<Self>) {
        if !self.mihomo_running()
            || self.profile_actions_locked()
            || (mode == self.mode
                && self
                    .baseline
                    .as_ref()
                    .is_some_and(|b| b.mode == mode.to_controller()))
        {
            return;
        }
        let Some(controller) = self.controller() else {
            return;
        };
        self.mode_switching = true;
        self.proxy_error = None;
        let generation = self.core_start_generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let target = mode.to_controller();
            let result = cx
                .background_executor()
                .spawn(async move { controller.patch_mode(target.as_str()) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.mode_switching = false;
                cx.notify();
                // 停机或重启后的旧请求不能保存为新内核的用户选择。
                if generation != this.core_start_generation || !this.mihomo_running() {
                    return;
                }
                match result {
                    Ok(()) => {
                        this.mode = mode;
                        if let Err(error) = this.save_mode_configuration(target) {
                            log_warn!("proxy", "运行模式已生效，但保存失败：{error:#}");
                            this.proxy_error = Some(t!("proxy.mode_save_failed").into_owned());
                        }
                        // GLOBAL 组的可见性随模式变化，重新拉取分组。
                        this.fetch_runtime_state(cx);
                    }
                    Err(error) => {
                        log_warn!("proxy", "切换运行模式失败：{error:#}");
                        this.proxy_error = Some(t!("proxy.mode_switch_failed").into_owned());
                    }
                }
            });
        })
        .detach();
    }

    /// 内核确认选择成功后立即持久化完整分组名和节点名，失败不覆盖旧记录。
    pub(super) fn select_node(&mut self, group_index: usize, node_index: usize, cx: &mut Context<Self>) {
        if self.profile_actions_locked() || !self.mihomo_running() {
            return;
        }
        let Some(group) = self.groups.get(group_index) else {
            return;
        };
        let Some(node) = group.nodes.get(node_index) else {
            return;
        };
        if !group.selectable
            || (group.now == node.name
                && self.config.selected_nodes.get(&group.name) == Some(&node.name)
                && self.proxy_error.is_none())
        {
            return;
        }
        let group_name = group.name.clone();
        let node_name = node.name.clone();
        let Some(controller) = self.controller() else {
            return;
        };

        self.node_switching = true;
        self.proxy_error = None;
        let generation = self.core_start_generation;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let selected_group = group_name.clone();
            let selected_node = node_name.clone();
            let result = cx
                .background_executor()
                .spawn(async move { controller.select_proxy(&group_name, &node_name) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.node_switching = false;
                cx.notify();
                if generation != this.core_start_generation || !this.mihomo_running() {
                    // 新内核可能在旧请求返回前已就绪，需要补拉取被忙态跳过的快照。
                    this.fetch_runtime_state(cx);
                    return;
                }
                match result {
                    Ok(()) => {
                        if let Some(group) = this
                            .groups
                            .iter_mut()
                            .find(|group| group.name == selected_group)
                        {
                            group.now = selected_node.clone();
                        }
                        // 保存失败仍保留当前会话的选择，并允许再次点击当前节点重试落盘。
                        this.config
                            .selected_nodes
                            .insert(selected_group, selected_node);
                        if let Err(error) = this.config.save(&this.paths.config_file) {
                            log_warn!("proxy", "节点已切换，但保存选择失败：{error:#}");
                            this.proxy_error = Some(t!("proxy.node_save_failed").into_owned());
                        }
                    }
                    Err(error) => {
                        log_warn!("proxy", "选择节点失败：{error:#}");
                        this.proxy_error = Some(t!("proxy.node_switch_failed").into_owned());
                        this.fetch_runtime_state(cx);
                    }
                }
            });
        })
        .detach();
    }
}
