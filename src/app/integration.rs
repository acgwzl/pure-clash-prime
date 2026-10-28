//! 系统集成：登录自启、系统代理接管与还原、托盘文案、代理状态落盘。
//!
//! 重建：源码在 rewind 快照中丢失，方法实现取自上游 app/mod.rs 的同名方法，
//! 随后按本仓库的字段与调用点修正。

#![allow(unused_imports, dead_code)]

use super::*;

impl PureClash {
    /// 生成当前语言下的托盘状态文案；托盘所有权由应用级 AppShell 持有。
    pub(super) fn tray_texts(&self) -> TrayTexts {
        let core = match self.core_state {
            CoreState::Running => t!("tray.running"),
            CoreState::Starting => t!("app.core_starting"),
            CoreState::Stopped => t!("tray.stopped"),
        };
        let system_proxy = if self.system_proxy {
            t!("tray.on")
        } else {
            t!("tray.off")
        };
        let tun = match self.tun_switch_state() {
            SwitchState::On => t!("tray.on"),
            SwitchState::Starting => t!("status.starting"),
            SwitchState::Off => t!("tray.off"),
        };
        TrayTexts {
            tooltip: t!(
                "tray.tooltip",
                core = core,
                system_proxy = system_proxy,
                tun = tun
            )
            .into_owned(),
            open: t!("tray.menu_open").into_owned(),
            quit: t!("tray.menu_quit").into_owned(),
        }
    }

    /// 登录自启开关直接修改平台配置；只有系统调用成功后才更新界面状态。
    pub(super) fn toggle_autostart(&mut self, cx: &mut Context<Self>) {
        if !self.autostart_available {
            return;
        }
        let enabled = !self.autostart_enabled;
        match set_autostart(enabled) {
            Ok(()) => {
                self.autostart_enabled = enabled;
                log_info!(
                    "autostart",
                    "登录自启已{}",
                    if enabled { "开启" } else { "关闭" }
                );
                self.integration_error = None;
            }
            Err(error) => {
                log_error!("autostart", "设置登录自启失败：{error:#}");
                self.integration_error = Some(
                    t!(
                        "app.autostart_failed",
                        error = concise_error(&format!("{error:#}"), 160)
                    )
                    .into_owned(),
                );
            }
        }
        cx.notify();
    }

    /// 每次进入设置页重新读取平台注册，外部启动管理器的修改不会被缓存状态覆盖。
    pub(super) fn refresh_autostart_status(&mut self) {
        match autostart_status() {
            Ok(AutoStartStatus::Enabled) => {
                self.autostart_enabled = true;
                self.autostart_available = true;
            }
            Ok(AutoStartStatus::Disabled) => {
                self.autostart_enabled = false;
                self.autostart_available = true;
            }
            Ok(AutoStartStatus::Unavailable) => {
                self.autostart_enabled = false;
                self.autostart_available = false;
            }
            Err(error) => {
                // 短暂读取失败时保留上次状态，避免把已启用误画成关闭。
                log_warn!("autostart", "刷新登录自启状态失败：{error:#}");
            }
        }
    }

    /// 用户明确打开窗口后，Linux 后续启用 TUN 可以弹出 polkit；登录自启本身
    /// 已经启动普通内核或通过现有服务启动 TUN，不在这里补发内核启动。
    pub(super) fn allow_interactive_elevation(&mut self) {
        self.interactive_elevation_allowed = true;
    }

    /// 开关系统代理：启用要求内核运行中；关闭恢复托管前的用户设置。
    pub(super) fn toggle_system_proxy(&mut self, cx: &mut Context<Self>) {
        if !self.system_proxy {
            if !self.mihomo_running() {
                self.integration_error = Some(t!("app.system_proxy_requires_core").into_owned());
                cx.notify();
                return;
            }
            match self.enable_system_proxy_endpoints() {
                Ok(()) => {
                    self.system_proxy = true;
                    if let Err(error) = self.config.save(&self.paths.config_file) {
                        log_error!("config", "保存系统代理配置失败：{error:#}");
                    }
                }
                Err(error) => {
                    log_error!("proxy", "开启系统代理失败：{error:#}");
                    self.integration_error = Some(system_proxy_error(&error));
                }
            }
        } else {
            self.disable_system_proxy();
            self.system_proxy = false;
            if let Err(error) = self.config.save(&self.paths.config_file) {
                log_error!("config", "保存系统代理配置失败：{error:#}");
            }
        }
        cx.notify();
    }

    /// 关闭系统代理并恢复托管前的用户设置；停内核与退出应用共用。
    pub(super) fn disable_system_proxy(&mut self) {
        if !self.system_proxy {
            return;
        }
        let snapshot = load_system_proxy_state(&self.paths).unwrap_or_default();
        match restore_system_proxy(&snapshot) {
            Ok(()) => {
                clear_system_proxy_state(&self.paths);
                log_info!("proxy", "系统代理已关闭并恢复用户原有设置");
                self.system_proxy = false;
                self.integration_error = None;
            }
            Err(error) => {
                // 恢复失败意味着用户系统设置可能残留 Pure Clash 代理地址，
                // 必须完整留痕供排查。
                log_error!("proxy", "恢复用户原有系统代理设置失败：{error:#}");
                self.integration_error = Some(system_proxy_error(&error));
            }
        }
    }

    /// 启用系统代理并保存托管前的系统设置快照。
    pub(super) fn enable_system_proxy_endpoints(&mut self) -> anyhow::Result<()> {
        let (http_addr, socks_addr) = self.current_proxy_endpoints();
        log_info!(
            "proxy",
            "开启系统代理（HTTP: {http_addr}, SOCKS: {:?}）",
            socks_addr
        );
        enable_system_proxy(&self.paths, &http_addr)?;
        self.integration_error = None;
        self.system_proxy = true;
        Ok(())
    }


    /// 获取当前系统代理的 HTTP 地址与可选的 SOCKS 地址（优先使用自定义监听配置）。
    pub(super) fn current_proxy_endpoints(&self) -> (String, Option<String>) {
        let mixed_port = self.baseline.as_ref().map_or(7890, |b| b.mixed_port);
        let mut http_port = None;
        let mut socks_port = None;

        for l in self.baseline.as_ref().map_or(&[][..], |b| b.listeners.as_slice()) {
            match l.listener_type.as_str() {
                "http" if http_port.is_none() => http_port = Some(l.port),
                "socks" if socks_port.is_none() => socks_port = Some(l.port),
                "mixed" => {
                    if http_port.is_none() {
                        http_port = Some(l.port);
                    }
                    if socks_port.is_none() {
                        socks_port = Some(l.port);
                    }
                }
                _ => {}
            }
        }

        let http = format!("127.0.0.1:{}", http_port.unwrap_or(mixed_port));
        let socks = socks_port.map(|p| format!("127.0.0.1:{p}"));
        (http, socks)
    }

    /// 内核就绪后按配置恢复系统代理设置（如上次退出前用户开启了系统代理）。
    pub(super) fn restore_system_proxy_if_configured(&mut self) {
        if self.system_proxy && !self.system_proxy {
            log_info!("proxy", "检测到配置中开启了系统代理，内核就绪后自动应用");
            if let Err(error) = self.enable_system_proxy_endpoints() {
                log_error!("proxy", "启动时自动开启系统代理失败：{error:#}");
                self.integration_error = Some(system_proxy_error(&error));
            }
        }
    }

/// 启用系统代理，捕获并返回托管前的用户设置快照。
fn enable_system_proxy(
    paths: &AppPaths,
    http_server: &str,
    socks_server: Option<&str>,
) -> anyhow::Result<()> {
    let snapshot = capture_system_proxy().inspect_err(|error| {
        log_error!("proxy", "捕获用户既有系统代理设置失败：{error:#}");
    })?;
    // 状态必须先于系统设置落盘；应用在 set_system_proxy 后崩溃时，下次启动
    // 仍能恢复用户原配置。
    save_system_proxy_state(paths, &snapshot).inspect_err(|error| {
        log_error!("proxy", "系统代理托管状态文件落盘失败：{error:#}");
    })?;
    if let Err(apply_error) = crate::platform::set_system_proxy_endpoints(http_server, socks_server) {
        log_error!("proxy", "写入系统代理设置失败：{apply_error:#}");
        match restore_system_proxy(&snapshot) {
            Ok(()) => clear_system_proxy_state(paths),
            Err(restore_error) => {
                log_error!(
                    "proxy",
                    "写入失败后自动恢复原系统代理也失败：{restore_error:#}"
                );
                return Err(anyhow::anyhow!(
                    "{apply_error:#}；自动恢复原系统代理也失败：{restore_error:#}"
                ));
            }
        }
        return Err(apply_error);
    }
    Ok(())
}

fn system_proxy_state_file(paths: &AppPaths) -> std::path::PathBuf {
    paths.data_dir.join("system-proxy.json")
}

/// 订阅日志只保留 host（含可选端口），userinfo 与完整 URL 均不进入日志。
fn host_of_url(url: &str) -> &str {
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority)
}

fn save_system_proxy_state(paths: &AppPaths, snapshot: &SystemProxySnapshot) -> anyhow::Result<()> {
    std::fs::create_dir_all(&paths.data_dir)?;
    let bytes = serde_json::to_vec_pretty(snapshot)?;
    let file = system_proxy_state_file(paths);
    let temporary = paths.data_dir.join("system-proxy.json.tmp");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(temporary, file)?;
    Ok(())
}

fn load_system_proxy_state(paths: &AppPaths) -> Option<SystemProxySnapshot> {
    let content = std::fs::read_to_string(system_proxy_state_file(paths)).ok()?;
    serde_json::from_str(&content).ok()
}

fn clear_system_proxy_state(paths: &AppPaths) {
    let _ = std::fs::remove_file(system_proxy_state_file(paths));
}

/// 系统代理操作失败的界面文案。
fn system_proxy_error(error: &anyhow::Error) -> String {
    t!(
        "app.system_proxy_failed",
        error = concise_error(&format!("{error:#}"), 160)
    )
    .into_owned()
}

}
