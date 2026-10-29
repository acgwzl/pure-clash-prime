//! 概览页：状态面板、上传/下载分卡、出站模式与系统开关、运行信息四卡、
//! 实时连接与策略组。
//!
//! 信息架构对齐主流 Mihomo 客户端的仪表盘：
//! 第一行回答「通不通、快不快」，第二行回答「怎么分流、接管没有」，
//! 第三行回答「配置/服务/资源现状」，第四行回答「现在在连什么」。

use gpui::{AnyElement, Context, Div, SharedString, Styled, div, px, relative};

use super::*;
use crate::assets::{ICON_CHART, ICON_CHEVRON_RIGHT, ICON_PLAY, ICON_STOP};
use crate::theme::{FontWeightExt, Palette, TabularNums, text_size};
use crate::ui::kit;

pub(super) fn render_overview(
    app: &PureClash,
    palette: Palette,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    div()
        .p_6()
        .flex()
        .flex_col()
        .gap_4()
        .when_some(app.integration_error.as_ref(), |page, error| {
            page.child(integration_error_banner(error, palette))
        })
        .child(top_row(app, palette, cx))
        .child(control_row(app, palette, cx))
        .child(info_row(app, palette, cx))
        .child(bottom_row(app, palette, cx))
        .into_any_element()
}

// ---------------------------------------------------------------- 第一行

/// 第一行：状态面板（2 份）+ 上传 + 下载（各 1 份）。
fn top_row(app: &PureClash, palette: Palette, cx: &mut Context<PureClash>) -> AnyElement {
    div()
        .flex()
        .gap_4()
        .h(px(196.0))
        .flex_none()
        .child(
            status_panel(app, palette, cx)
                .flex_grow(2.0)
                .flex_basis(relative(0.0))
                .min_w_0(),
        )
        .child(speed_card(app, palette, SpeedKind::Up).flex_1().min_w_0())
        .child(speed_card(app, palette, SpeedKind::Down).flex_1().min_w_0())
        .into_any_element()
}

/// 状态面板：运行状态、出口地址与运营商、启动/停止按钮、运行时长与当前配置。
fn status_panel(app: &PureClash, palette: Palette, cx: &mut Context<PureClash>) -> Div {
    let running = app.core_state == CoreState::Running;
    let starting = app.core_state == CoreState::Starting;
    let state_color = if running {
        palette.success
    } else if starting {
        palette.amber
    } else {
        palette.faint
    };
    let state_text = if running {
        tr("overview.running")
    } else if starting {
        tr("overview.starting")
    } else {
        tr("overview.not_started")
    };

    kit::hero_card(palette)
        .h_full()
        .p(px(22.0))
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(kit::status_dot(palette, state_color))
                .child(
                    div()
                        .text_size(px(text_size::META))
                        .text_color(palette.muted)
                        .child(state_text),
                ),
        )
        .child(
            div()
                .mt_3()
                .flex()
                .items_center()
                .gap_4()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .font_family("Bahnschrift")
                                .text_size(px(30.0))
                                .font_semibold()
                                .tabnum()
                                .text_color(palette.text)
                                .truncate()
                                .child(public_ip_text(app)),
                        )
                        .child(
                            div()
                                .mt_1()
                                .flex()
                                .items_center()
                                .gap_2()
                                .min_w_0()
                                .when(!app.net_check_flag.is_empty(), |row| {
                                    row.child(
                                        div()
                                            .flex_none()
                                            .text_size(px(text_size::META))
                                            .child(SharedString::from(app.net_check_flag.clone())),
                                    )
                                })
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(text_size::META))
                                        .text_color(palette.muted)
                                        .child(isp_text(app)),
                                ),
                        ),
                )
                .child(power_button(app, palette, cx)),
        )
        .child(
            div()
                .mt_auto()
                .pt_3()
                .flex()
                .items_center()
                .gap_6()
                .child(meta_pair(tr("overview.uptime"), uptime_text(app), palette))
                .child(meta_pair(
                    tr("overview.active_profile"),
                    active_profile_name(app),
                    palette,
                ))
                .child(meta_pair(
                    tr("overview.active_node"),
                    active_node_name(app),
                    palette,
                )),
        )
}

/// 大圆启动按钮：未运行时是品牌色的播放，运行中是危险色的停止。
fn power_button(app: &PureClash, palette: Palette, cx: &mut Context<PureClash>) -> AnyElement {
    let running = app.core_state == CoreState::Running;
    let starting = app.core_state == CoreState::Starting;
    let color = if running {
        palette.danger
    } else if starting {
        palette.amber
    } else {
        palette.brand
    };
    div()
        .id("overview-power")
        .size(px(58.0))
        .flex_none()
        .rounded_full()
        .bg(color)
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|style| style.opacity(0.86))
        .child(icon(
            if running { ICON_STOP } else { ICON_PLAY },
            palette.knob,
            22.0,
        ))
        .on_click(cx.listener(|this, _, _, cx| this.toggle_core(cx)))
        .into_any_element()
}

fn meta_pair(label: SharedString, value: SharedString, palette: Palette) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_size(px(text_size::HINT))
                .text_color(palette.faint)
                .child(label),
        )
        .child(
            div()
                .font_family("Bahnschrift")
                .text_size(px(text_size::BODY))
                .tabnum()
                .text_color(palette.text)
                .truncate()
                .child(value),
        )
        .into_any_element()
}

#[derive(Clone, Copy)]
enum SpeedKind {
    Up,
    Down,
}

/// 上传/下载分卡：峰值、当前速率、累计。
fn speed_card(app: &PureClash, palette: Palette, kind: SpeedKind) -> Div {
    let (speed, peak, total, label, color) = match kind {
        SpeedKind::Up => (
            app.traffic_up_speed,
            app.peak_up_speed,
            app.upload_total,
            tr("overview.upload"),
            palette.amber,
        ),
        SpeedKind::Down => (
            app.traffic_down_speed,
            app.peak_down_speed,
            app.download_total,
            tr("overview.download"),
            palette.success,
        ),
    };
    let (num, unit) = split_speed(speed);

    kit::card(palette)
        .h_full()
        .p(px(20.0))
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .min_w_0()
                        .child(icon(ICON_CHART, color, 14.0))
                        .child(
                            div()
                                .text_size(px(text_size::LABEL))
                                .text_color(palette.muted)
                                .child(label),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(text_size::HINT))
                        .text_color(palette.faint)
                        .child(SharedString::from(format!(
                            "{} {}",
                            tr("overview.peak"),
                            format_speed(peak)
                        ))),
                ),
        )
        .child(
            div()
                .mt_auto()
                .flex()
                .items_baseline()
                .gap_1()
                .child(
                    div()
                        .font_family("Bahnschrift")
                        .text_size(px(34.0))
                        .font_semibold()
                        .tabnum()
                        .text_color(palette.text)
                        .child(num),
                )
                .child(
                    div()
                        .text_size(px(text_size::META))
                        .text_color(palette.muted)
                        .child(unit),
                ),
        )
        .child(
            div()
                .mt_2()
                .text_size(px(text_size::HINT))
                .text_color(palette.faint)
                .child(SharedString::from(format!(
                    "{} {}",
                    tr("overview.total"),
                    format_bytes(total)
                ))),
        )
}

// ---------------------------------------------------------------- 第二行

/// 第二行：出站模式三选一 + 系统代理 + TUN。
fn control_row(app: &PureClash, palette: Palette, cx: &mut Context<PureClash>) -> AnyElement {
    div()
        .flex()
        .gap_4()
        .flex_none()
        .child(mode_switcher(app, palette, cx))
        .child(switch_chip(
            "overview-system-proxy",
            tr("overview.system_proxy"),
            SwitchState::from(app.system_proxy),
            palette,
            cx.listener(|this, _, _, cx| this.toggle_system_proxy(cx)),
        ))
        .child(switch_chip(
            "overview-tun",
            tr("overview.virtual_nic"),
            tun_visible_state(app.core_state, app.tun_state),
            palette,
            cx.listener(|this, _, _, cx| this.toggle_tun(cx)),
        ))
        .into_any_element()
}

fn mode_switcher(app: &PureClash, palette: Palette, cx: &mut Context<PureClash>) -> AnyElement {
    kit::card(palette)
        .h(px(52.0))
        .px(px(14.0))
        .flex()
        .items_center()
        .gap_3()
        .child(
            div()
                .flex_none()
                .text_size(px(text_size::LABEL))
                .text_color(palette.faint)
                .child(tr("overview.outbound_mode")),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .p_1()
                .rounded_lg()
                .bg(palette.inset)
                .child(mode_button(app, palette, cx, ProxyMode::Rule))
                .child(mode_button(app, palette, cx, ProxyMode::Global))
                .child(mode_button(app, palette, cx, ProxyMode::Direct)),
        )
        .into_any_element()
}

fn mode_button(
    app: &PureClash,
    palette: Palette,
    cx: &mut Context<PureClash>,
    mode: ProxyMode,
) -> AnyElement {
    let active = app.mode == mode;
    let id = match mode {
        ProxyMode::Rule => "mode-rule",
        ProxyMode::Global => "mode-global",
        ProxyMode::Direct => "mode-direct",
    };
    div()
        .id(id)
        .px(px(12.0))
        .py(px(5.0))
        .rounded_md()
        .cursor_pointer()
        .when(active, |button| {
            button.bg(palette.brand).text_color(palette.knob)
        })
        .when(!active, |button| {
            button
                .text_color(palette.muted)
                .hover(|style| style.text_color(palette.text))
        })
        .text_size(px(text_size::ROW))
        .font_medium()
        .child(mode.label())
        .on_click(cx.listener(move |this, _, _, cx| this.set_mode(mode, cx)))
        .into_any_element()
}

fn switch_chip(
    id: &'static str,
    label: SharedString,
    state: SwitchState,
    palette: Palette,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let on = state == SwitchState::On;
    div()
        .id(id)
        .h(px(52.0))
        .px(px(18.0))
        .flex()
        .items_center()
        .gap_3()
        .rounded_xl()
        .bg(palette.surface)
        .border_1()
        .border_color(palette.border)
        .cursor_pointer()
        .hover(|style| style.border_color(palette.brand_line))
        .child(
            div()
                .flex_none()
                .text_size(px(text_size::ROW))
                .text_color(palette.text)
                .child(label),
        )
        .child(toggle_knob(palette, on, state == SwitchState::Starting))
        .on_click(handler)
        .into_any_element()
}

fn toggle_knob(palette: Palette, on: bool, pending: bool) -> AnyElement {
    let bg = if on {
        palette.brand
    } else if pending {
        palette.amber
    } else {
        palette.off
    };
    div()
        .w(px(38.0))
        .h(px(21.0))
        .flex_none()
        .rounded_full()
        .bg(bg)
        .flex()
        .items_center()
        .when(on, |track| track.justify_end())
        .when(!on, |track| track.justify_start())
        .child(
            div()
                .m(px(2.0))
                .size(px(17.0))
                .rounded_full()
                .bg(palette.knob),
        )
        .into_any_element()
}

// ---------------------------------------------------------------- 第三行

/// 第三行：网络、当前配置、服务状态、内存四张信息卡。
fn info_row(app: &PureClash, palette: Palette, cx: &mut Context<PureClash>) -> AnyElement {
    div()
        .flex()
        .gap_4()
        .h(px(128.0))
        .flex_none()
        .child(network_card(app, palette, cx).flex_1().min_w_0())
        .child(div().flex_1().min_w_0().child(profile_card(app, palette, cx)))
        .child(service_card(app, palette).flex_1().min_w_0())
        .child(memory_card(app, palette).flex_1().min_w_0())
        .into_any_element()
}

fn info_card_shell(palette: Palette) -> Div {
    kit::card(palette).h_full().p(px(18.0)).flex().flex_col()
}

fn card_head(icon_path: &'static str, title: SharedString, palette: Palette) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(icon(icon_path, palette.faint, 13.0))
        .child(
            div()
                .text_size(px(text_size::LABEL))
                .text_color(palette.muted)
                .child(title),
        )
        .into_any_element()
}

fn network_card(app: &PureClash, palette: Palette, cx: &mut Context<PureClash>) -> Div {
    let local = app
        .net_check_local
        .clone()
        .map(SharedString::from)
        .unwrap_or_else(|| SharedString::from("—"));

    info_card_shell(palette)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(card_head(ICON_GIT_BRANCH, tr("overview.network"), palette))
                .child(
                    div()
                        .id("overview-net-check")
                        .px(px(8.0))
                        .py(px(2.0))
                        .rounded_md()
                        .bg(palette.inset)
                        .text_size(px(text_size::TINY))
                        .text_color(palette.muted)
                        .cursor_pointer()
                        .hover(|style| style.text_color(palette.text))
                        .child(if app.net_check_loading {
                            tr("overview.network_checking")
                        } else {
                            tr("overview.network_check_button")
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.check_network(cx))),
                ),
        )
        .child(
            div()
                .mt_auto()
                .font_family("Bahnschrift")
                // 内网地址最长 15 字符，四卡并排时 25px 会截断，降到 19px。
                .text_size(px(19.0))
                .font_semibold()
                .tabnum()
                .text_color(palette.text)
                .truncate()
                .child(local),
        )
        .child(
            div()
                .mt_1()
                .text_size(px(text_size::HINT))
                .text_color(palette.faint)
                .child(tr("overview.network_local")),
        )
}

fn profile_card(
    app: &PureClash,
    palette: Palette,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    let name = active_profile_name(app);
    let groups = app.groups.len();
    let nodes: usize = app.groups.iter().map(|group| group.nodes.len()).sum();
    let rules = app.rules.len();

    info_card_shell(palette)
        .id("overview-profile-card")
        .cursor_pointer()
        .hover(|style| style.border_color(palette.brand_line))
        .on_click(cx.listener(|this, _, _, cx| this.select_page(Page::Profiles, cx)))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(card_head(
                    ICON_FILE_CODE,
                    tr("overview.active_profile"),
                    palette,
                ))
                .child(icon(ICON_CHEVRON_RIGHT, palette.faint, 13.0)),
        )
        .child(
            div()
                .mt_auto()
                .text_size(px(text_size::STAT_NUM))
                .font_semibold()
                .text_color(palette.text)
                .truncate()
                .child(name),
        )
        .child(
            // 三项并排 + 换行会散成两截，改成单行紧凑写法，超出即省略。
            div()
                .mt_1()
                .min_w_0()
                .truncate()
                .text_size(px(text_size::TINY))
                .text_color(palette.faint)
                .child(SharedString::from(format!(
                    "{} {} · {} {} · {} {}",
                    groups,
                    tr("overview.groups"),
                    nodes,
                    tr("overview.nodes"),
                    rules,
                    tr("overview.rules"),
                ))),
        )
        .into_any_element()
}

fn count_pair(value: String, label: SharedString, palette: Palette) -> AnyElement {
    div()
        .flex()
        .items_baseline()
        .gap_1()
        .child(
            div()
                .font_family("Bahnschrift")
                .text_size(px(text_size::META))
                .tabnum()
                .text_color(palette.text)
                .child(SharedString::from(value)),
        )
        .child(
            div()
                .text_size(px(text_size::TINY))
                .text_color(palette.faint)
                .child(label),
        )
        .into_any_element()
}

fn service_card(app: &PureClash, palette: Palette) -> Div {
    let running = app.core_state == CoreState::Running;
    let controller = app
        .baseline
        .as_ref()
        .map(|baseline| baseline.controller_addr.clone())
        .unwrap_or_default();
    let pid = app
        .mihomo_process
        .as_ref()
        .and_then(|process| process.pid())
        .map(|pid| pid.to_string())
        .unwrap_or_else(|| "—".to_owned());

    info_card_shell(palette)
        .child(card_head(ICON_SHIELD_CHECK, tr("overview.service"), palette))
        .child(
            div()
                .mt_auto()
                .flex()
                .items_center()
                .gap_2()
                .child(kit::status_dot(
                    palette,
                    if running {
                        palette.success
                    } else {
                        palette.faint
                    },
                ))
                .child(
                    div()
                        .text_size(px(text_size::BODY))
                        .text_color(palette.text)
                        .child(if running {
                            tr("overview.running")
                        } else {
                            tr("overview.not_started")
                        }),
                ),
        )
        .child(
            div()
                .mt_1()
                .font_family("Bahnschrift")
                .text_size(px(text_size::HINT))
                .tabnum()
                .text_color(palette.faint)
                .truncate()
                .child(SharedString::from(if controller.is_empty() {
                    format!("pid {pid}")
                } else {
                    format!("{controller} · pid {pid}")
                })),
        )
}

fn memory_card(app: &PureClash, palette: Palette) -> Div {
    let total = match (app.app_memory, app.core_memory) {
        (Some(app_bytes), Some(core_bytes)) => Some(app_bytes + core_bytes),
        (Some(app_bytes), None) => Some(app_bytes),
        (None, Some(core_bytes)) => Some(core_bytes),
        (None, None) => None,
    };
    info_card_shell(palette)
        .child(card_head(ICON_CHART, tr("overview.memory"), palette))
        .child(
            div()
                .mt_auto()
                .font_family("Bahnschrift")
                .text_size(px(22.0))
                .font_semibold()
                .tabnum()
                .text_color(palette.text)
                .truncate()
                .child(memory_text(total)),
        )
        .child(
            div()
                .mt_1()
                .flex()
                .items_center()
                .gap_3()
                .child(count_pair(
                    memory_text(app.app_memory).to_string(),
                    tr("overview.app_memory"),
                    palette,
                ))
                .child(count_pair(
                    memory_text(app.core_memory).to_string(),
                    tr("overview.core_memory"),
                    palette,
                )),
        )
}

// ---------------------------------------------------------------- 第四行

/// 第四行：实时连接（2 份）+ 策略组（1 份）。
fn bottom_row(app: &PureClash, palette: Palette, cx: &mut Context<PureClash>) -> AnyElement {
    div()
        .flex_1()
        .min_h(px(180.0))
        .flex()
        .gap_4()
        .child(
            connections_card(app, palette, cx)
                .flex_grow(2.0)
                .flex_basis(relative(0.0))
                .min_w_0(),
        )
        .child(div().flex_1().min_w_0().child(groups_card(app, palette, cx)))
        .into_any_element()
}

fn connections_card(app: &PureClash, palette: Palette, cx: &mut Context<PureClash>) -> Div {
    let rows: Vec<AnyElement> = app
        .connections
        .iter()
        .take(5)
        .map(|item| connection_row(item, palette))
        .collect();

    kit::card(palette)
        .h_full()
        .p(px(18.0))
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(card_head(
                    ICON_MESSAGE,
                    tr("overview.active_connections"),
                    palette,
                ))
                .child(
                    div()
                        .text_size(px(text_size::HINT))
                        .text_color(palette.faint)
                        .child(SharedString::from(app.connections.len().to_string())),
                ),
        )
        .child(
            div()
                .mt_3()
                .flex()
                .items_center()
                .gap_3()
                .text_size(px(text_size::TINY))
                .text_color(palette.faint)
                .child(div().flex_1().min_w_0().child(tr("overview.col_target")))
                .child(div().w(px(90.0)).flex_none().child(tr("overview.col_outbound")))
                .child(div().w(px(62.0)).flex_none().child(tr("overview.col_download")))
                .child(div().w(px(62.0)).flex_none().child(tr("overview.col_upload"))),
        )
        .child(
            div()
                .mt_2()
                .flex_1()
                .min_h(px(0.0))
                .flex()
                .flex_col()
                .when(rows.is_empty(), |list| {
                    list.child(
                        div()
                            .mt_2()
                            .text_size(px(text_size::HINT))
                            .text_color(palette.faint)
                            .child(tr("overview.no_connections")),
                    )
                })
                .children(rows),
        )
        .when(app.connections.len() > 5, |card| {
            card.child(
                div()
                    .id("overview-more-connections")
                    .pt_2()
                    .text_size(px(text_size::TINY))
                    .text_color(palette.faint)
                    .cursor_pointer()
                    .hover(|style| style.text_color(palette.text))
                    .child(tr("overview.more_connections"))
                    .on_click(cx.listener(|this, _, _, cx| this.select_page(Page::Connections, cx))),
            )
        })
}

fn connection_row(
    item: &crate::mihomo::controller::ConnectionItem,
    palette: Palette,
) -> AnyElement {
    let target = if !item.metadata.host.is_empty() {
        format!("{}:{}", item.metadata.host, item.metadata.destination_port)
    } else if !item.metadata.destination_ip.is_empty() {
        format!(
            "{}:{}",
            item.metadata.destination_ip, item.metadata.destination_port
        )
    } else {
        "—".to_owned()
    };
    let outbound = item
        .chains
        .first()
        .cloned()
        .unwrap_or_else(|| "—".to_owned());

    div()
        .py(px(5.0))
        .flex()
        .items_center()
        .gap_3()
        .text_size(px(text_size::HINT))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(palette.text)
                .child(SharedString::from(target)),
        )
        .child(
            div()
                .w(px(90.0))
                .flex_none()
                .truncate()
                .text_color(palette.muted)
                .child(SharedString::from(outbound)),
        )
        .child(
            div()
                .w(px(62.0))
                .flex_none()
                .font_family("Bahnschrift")
                .tabnum()
                .text_color(palette.success)
                .child(SharedString::from(format_bytes(item.download))),
        )
        .child(
            div()
                .w(px(62.0))
                .flex_none()
                .font_family("Bahnschrift")
                .tabnum()
                .text_color(palette.amber)
                .child(SharedString::from(format_bytes(item.upload))),
        )
        .into_any_element()
}

fn groups_card(app: &PureClash, palette: Palette, cx: &mut Context<PureClash>) -> AnyElement {
    // 承载流量的组优先展示（rules 的 MATCH 目标），其余按原顺序补齐。
    let primary = active_group(app).map(|group| group.name.clone());
    let mut ordered: Vec<&crate::mihomo::controller::GroupSnapshot> = Vec::new();
    if let Some(name) = primary.as_deref() {
        if let Some(group) = app.groups.iter().find(|group| group.name == name) {
            ordered.push(group);
        }
    }
    for group in app.groups.iter() {
        if ordered.iter().any(|existing| existing.name == group.name) {
            continue;
        }
        if ordered.len() >= 4 {
            break;
        }
        ordered.push(group);
    }
    let rows: Vec<AnyElement> = ordered
        .into_iter()
        .map(|group| group_row(group, palette))
        .collect();

    kit::card(palette)
        .id("overview-groups-card")
        .h_full()
        .p(px(18.0))
        .flex()
        .flex_col()
        .cursor_pointer()
        .hover(|style| style.border_color(palette.brand_line))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(card_head(ICON_GIT_BRANCH, tr("overview.groups"), palette))
                .child(icon(ICON_CHEVRON_RIGHT, palette.faint, 13.0)),
        )
        .child(
            div()
                .mt_3()
                .flex_1()
                .min_h(px(0.0))
                .flex()
                .flex_col()
                .gap_2()
                .when(rows.is_empty(), |list| {
                    list.child(
                        div()
                            .text_size(px(text_size::HINT))
                            .text_color(palette.faint)
                            .child(tr("overview.core_not_running")),
                    )
                })
                .children(rows),
        )
        .on_click(cx.listener(|this, _, _, cx| this.select_page(Page::Proxies, cx)))
        .into_any_element()
}

fn group_row(group: &crate::mihomo::controller::GroupSnapshot, palette: Palette) -> AnyElement {
    let initial = group
        .name
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default();

    div()
        .px(px(10.0))
        .py(px(7.0))
        .rounded_lg()
        .bg(palette.inset)
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .size(px(18.0))
                .flex_none()
                .rounded_full()
                .bg(palette.surface_high)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(px(text_size::TINY))
                        .text_color(palette.muted)
                        .child(SharedString::from(initial)),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_size(px(text_size::META))
                        .text_color(palette.text)
                        .truncate()
                        .child(SharedString::from(group.name.clone())),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .min_w_0()
                                .text_size(px(text_size::TINY))
                                .text_color(palette.faint)
                                .truncate()
                                .child(SharedString::from(group.now.clone())),
                        )
                        // url-test / fallback 由内核自动挑节点，不能手动指定。
                        .when(!group.selectable, |row| {
                            row.child(
                                div()
                                    .flex_none()
                                    .px(px(4.0))
                                    .rounded_sm()
                                    .bg(palette.inset)
                                    .text_size(px(text_size::TINY))
                                    .text_color(palette.muted)
                                    .child(tr("overview.group_auto")),
                            )
                        }),
                ),
        )
        .into_any_element()
}

// ---------------------------------------------------------------- 工具

/// 承载流量的策略组：优先按已知命名找，找不到时退到第一个可手动选择的组。
fn active_group(app: &PureClash) -> Option<&crate::mihomo::controller::GroupSnapshot> {
    const PREFERRED: [&str; 4] = ["proxy", "PROXY", "节点选择", "Proxy"];
    for name in PREFERRED {
        if let Some(group) = app.groups.iter().find(|group| group.name == name) {
            return Some(group);
        }
    }
    app.groups
        .iter()
        .find(|group| group.selectable)
        .or_else(|| app.groups.first())
}

/// 当前实际使用的节点：有活跃连接时以连接链首个节点为准，
/// 没有连接时退到承载流量策略组的选择结果。
fn active_node_name(app: &PureClash) -> SharedString {
    if let Some(node) = app
        .connections
        .iter()
        .find_map(|item| item.chains.first())
    {
        return SharedString::from(node.clone());
    }
    active_group(app)
        .map(|group| SharedString::from(group.now.clone()))
        .unwrap_or_else(|| tr("overview.no_selection"))
}

fn public_ip_text(app: &PureClash) -> SharedString {
    if app.net_check_loading {
        return tr("overview.network_checking");
    }
    app.net_check_public
        .clone()
        .map(SharedString::from)
        .unwrap_or_else(|| tr("overview.network_undetected"))
}

fn isp_text(app: &PureClash) -> SharedString {
    if let Some(error) = app.net_check_error.as_ref() {
        return SharedString::from(error.clone());
    }
    app.public_isp
        .clone()
        .map(SharedString::from)
        .unwrap_or_else(|| tr("overview.isp_unknown"))
}

fn uptime_text(app: &PureClash) -> SharedString {
    let Some(started) = app.core_started_at else {
        return SharedString::from("—");
    };
    let secs = started.elapsed().as_secs();
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;
    if hours > 0 {
        SharedString::from(format!("{hours:02}:{minutes:02}:{seconds:02}"))
    } else {
        SharedString::from(format!("{minutes:02}:{seconds:02}"))
    }
}

fn active_profile_name(app: &PureClash) -> SharedString {
    app.active_profile
        .as_ref()
        .and_then(|id| app.config.profiles.iter().find(|meta| &meta.id == id))
        .map(|meta| SharedString::from(meta.name.clone()))
        .unwrap_or_else(|| tr("overview.no_selection"))
}

fn memory_text(bytes: Option<u64>) -> SharedString {
    match bytes {
        Some(bytes) => SharedString::from(format!("{:.1} MB", bytes as f64 / 1024.0 / 1024.0)),
        None => SharedString::from("—"),
    }
}

fn split_speed(bytes_per_second: u64) -> (String, &'static str) {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes_per_second as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        (format!("{bytes_per_second}"), "B/s")
    } else {
        (
            format!("{value:.1}"),
            match unit {
                1 => "KB/s",
                2 => "MB/s",
                3 => "GB/s",
                _ => "TB/s",
            },
        )
    }
}

pub(super) fn integration_error_banner(error: &str, palette: Palette) -> AnyElement {
    div()
        .px(px(16.0))
        .py(px(10.0))
        .rounded_lg()
        .bg(palette.danger_soft)
        .border_1()
        .border_color(palette.danger)
        .text_size(px(text_size::META))
        .text_color(palette.danger)
        .child(SharedString::from(error.to_owned()))
        .into_any_element()
}
