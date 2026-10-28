//! TUN 模式：权限提升、开关流程、生效校验与回退。
//!
//! 重建：源码在 rewind 快照中丢失，方法实现取自上游 app/mod.rs 的同名方法，
//! 随后按本仓库的字段与调用点修正。

#![allow(unused_imports, dead_code)]

use super::*;

impl PureClash {
    /// TUN 只有在内核运行且 controller 已确认时才算真正开启；持久配置本身
    /// 不能用于状态展示，避免启动中或启动失败时出现虚假开启。
    pub(super) fn tun_running(&self) -> bool {
        self.tun_switch_state() == SwitchState::On
    }

    /// 所有页面和托盘共用同一个可见状态，避免启动过程中显示为关闭。
    pub(super) fn tun_switch_state(&self) -> SwitchState {
        tun_visible_state(self.core_state, self.tun_state)
    }

    /// 内核就绪后核对 TUN 是否真实生效。
    ///
    /// 内核在缺少系统网络权限或平台 TUN 能力时会静默降级继续运行，界面不能
    /// 假装 TUN 已开启：未生效时自动回退关闭并重启内核，明确提示原因。
    pub(super) fn verify_tun_effective(&mut self, generation: u64, cx: &mut Context<Self>) {
        if !self.tun_configured {
            return;
        }
        let Some(controller) = self.controller() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let background = cx.background_executor().clone();
            let result = async {
                // Windows 首次创建 Wintun 网卡可能超过 3 秒；初始化中返回的
                // tun.enable=false 不能立即判为失败。用单调时钟给足 30 秒，
                // 避免按次数重试时把每次 HTTP 超时叠加成数分钟；其他平台保持 3 秒。
                // 不记录完整响应，避免配置中的敏感字段进入日志。
                let started = std::time::Instant::now();
                let timeout = std::time::Duration::from_secs(if cfg!(target_os = "windows") {
                    30
                } else {
                    3
                });
                let mut attempt = 0;
                let mut last_log = std::time::Duration::ZERO;
                let mut last_error = None;
                log_info!("tun", "开始确认 TUN 状态（启动代次 {generation}，等待窗口 {} 秒，间隔 300ms）", timeout.as_secs());
                while started.elapsed() < timeout {
                    // 每轮检查代次：停止或切换配置后及时结束旧探针，避免等待期内
                    // 多个探针对新内核重复请求或写入误导日志。
                    let active = this.update(cx, |this, _| {
                        generation == this.core_start_generation && this.tun_state == SwitchState::Starting
                    }).unwrap_or(false);
                    if !active { return Ok(()); }
                    attempt += 1;
                    let controller = controller.clone();
                    let snapshot = background.spawn(async move { controller.configs() }).await;
                    let active = this.update(cx, |this, _| {
                        generation == this.core_start_generation && this.tun_state == SwitchState::Starting
                    }).unwrap_or(false);
                    if !active { return Ok(()); }
                    let elapsed = started.elapsed();
                    let log_progress = attempt == 1 || elapsed.saturating_sub(last_log) >= std::time::Duration::from_secs(5);
                    match snapshot {
                        Ok(config) => {
                            // 持续等待每 5 秒留一条进度，成功/错误恢复立即记录。
                            let recovered = last_error.take().is_some();
                            if config.tun_enabled || log_progress || recovered {
                                log_info!("tun", "TUN 状态检查 #{attempt}（代次 {generation}，累计 {}ms）：tun.enable={}", elapsed.as_millis(), config.tun_enabled);
                                last_log = elapsed;
                            }
                            if config.tun_enabled {
                                return Ok(());
                            }
                        }
                        Err(error) => {
                            let error = format!("{error:#}");
                            if log_progress || last_error.as_ref() != Some(&error) {
                                log_warn!("tun", "TUN 状态检查 #{attempt}（代次 {generation}，累计 {}ms）请求失败：{error}", elapsed.as_millis());
                                last_log = elapsed;
                            }
                            last_error = Some(error);
                        }
                    }
                    // 不在窗口尾部多等一次完整间隔；正在进行的 HTTP 请求仍受
                    // controller 的 3 秒超时约束，最坏总时长约为窗口加一次请求。
                    let remaining = timeout.saturating_sub(started.elapsed());
                    if !remaining.is_zero() {
                        background.timer(remaining.min(std::time::Duration::from_millis(300))).await;
                    }
                }
                log_warn!("tun", "TUN 确认窗口结束（代次 {generation}，累计 {}ms），Windows 初始化错误请检查 log/tun-kernel.log", started.elapsed().as_millis());
                anyhow::bail!("Mihomo controller 未确认 TUN 已生效")
            }.await;
            let _ = this.update(cx, |this, cx| {
                if generation != this.core_start_generation || this.tun_state != SwitchState::Starting {
                    return;
                }
                let Err(error) = result else {
                    // 只有 controller 明确返回开启，界面和托盘才显示 TUN 已生效。
                    log_info!("tun", "controller 确认 TUN 已生效");
                    this.tun_state = SwitchState::On;
                                        cx.notify();
                    return;
                };
                if !this.tun_configured {
                    return;
                }
                // 即使回退文件写入失败，也结束过渡态，允许用户重试。
                this.tun_state = SwitchState::Off;
                // 回退事务会校验并原子写入关闭 TUN 的配置，运行中自动重启内核。
                log_warn!("tun", "controller 未确认 TUN 生效，自动回退关闭：{error:#}");
                let message = tun_reverted_error(&error);
                // 挂在 integration_error 上：内核重启成功的就绪探针会清掉 mihomo_error。
                this.integration_error = Some(match this.revert_tun(cx) {
                    Ok(()) => message,
                    Err(revert_error) => {
                        log_error!("tun", "回退 TUN 配置失败：{revert_error:#}");
                        concise_error(&format!("{message}；回退配置失败：{revert_error:#}"), 220)
                    }
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// 回退 TUN：状态、基线与运行时配置同步关闭并持久化。
    ///
    /// 供真实停机路径调用（手动停止内核、启动失败、TUN 未生效回退）；
    /// 调用时内核应已停止或尚未拉起，配置事务因此只重写 runtime.yaml，
    /// 不会重启内核。配置切换重启与托盘退出不走这里，TUN 基线保持不变。
    pub(super) fn revert_tun(&mut self, cx: &mut Context<Self>) -> anyhow::Result<()> {
        self.apply_tun_configuration(false, cx)
    }

    /// 开关 TUN：写入本地基线并重新合并校验；内核运行中自动重启生效。
    ///
    /// TUN 由内核承载，内核未运行时不允许开启（与系统代理约束一致）；
    /// 授权和初始化期间不接受重复切换，仍可通过停止内核中止运行。
    pub(super) fn toggle_tun(&mut self, cx: &mut Context<Self>) {
        if !self.core_operable() || self.tun_switch_state() == SwitchState::Starting {
            return;
        }
        let enabled = !self.tun_configured;
        if enabled && !self.mihomo_running() {
            log_warn!("tun", "拒绝开启 TUN：内核未运行");
            self.integration_error = Some(t!("app.tun_requires_core").into_owned());
            cx.notify();
            return;
        }
        let Some(_) = self.baseline.as_ref() else {
            log_warn!("tun", "拒绝切换 TUN：本地基线不可用");
            self.integration_error = Some(t!("app.baseline_missing").into_owned());
            cx.notify();
            return;
        };
        if let Err(error) = self.apply_tun_configuration(enabled, cx) {
            log_error!("tun", "TUN 配置事务失败：{error:#}");
            self.integration_error = Some(concise_error(&format!("{error:#}"), 160));
            cx.notify();
            return;
        }
        log_info!(
            "tun",
            "TUN 配置已{}（内核重启后由 controller 确认真实生效）",
            if enabled { "开启" } else { "关闭" }
        );
        self.integration_error = None;
        cx.notify();
    }

    /// 先校验目标 TUN 配置，再提交基线和 runtime；任一写入失败均不重启当前内核。
    pub(super) fn apply_tun_configuration(
        &mut self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let previous = self
            .baseline
            .clone()
            .ok_or_else(|| anyhow::anyhow!("本地基线不可用"))?;
        let mut desired = previous.clone();
        desired.tun_enable = enabled;
        let runtime = profile::prepare_runtime(
            &self.paths,
            Some(&desired),
            &self.config.mihomo_version,
            self.active_profile.as_deref(),
        )?;

        save_baseline(&self.paths, &desired)?;
        if let Err(error) = write_runtime(&self.paths, &runtime) {
            let rollback = save_baseline(&self.paths, &previous);
            return match rollback {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(anyhow::anyhow!(
                    "{error:#}；恢复原本地基线也失败：{rollback_error:#}"
                )),
            };
        }

        self.baseline = Some(desired);
        self.tun_configured = enabled;
        // 配置提交不等于系统能力已经生效；重启后由 controller 再确认。
        self.tun_state = SwitchState::Off;
        if self.core_active() {
            self.restart_core(cx);
        }
        Ok(())
    }

/// TUN 自动回退时保留可操作的失败原因，但限制长度，避免平台命令输出撑坏横幅。
fn tun_reverted_error(error: &anyhow::Error) -> String {
    t!(
        "app.tun_reverted",
        error = concise_error(&format!("{error:#}"), 180)
    )
    .into_owned()
}
}
