//! 概览页：英雄区（当前出站 + 实时速率）、状态带、订阅用量 + 网络检测、运行状态条。
//!
//! 布局与浏览器原型 `web/overview.html` 对齐：
//! 英雄区回答「走哪、多快」，状态带回答「规模」，两卡回答「订阅与出口」，
//! 状态条回答「内核与集成」。原来独立成卡的运行时信息收进底部状态条。

use gpui::{AnyElement, Context, Rgba, SharedString, Styled, div, px};
use rust_i18n::t;

use super::net_check::render_network_card;
use super::*;
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
        .child(hero(app, palette, cx))
        .child(stats_strip(app, palette))
        .child(
            // 两张卡吃掉剩余高度；集成错误横幅出现时它们让位而不是把页面撑破。
            div()
                .flex_1()
                .min_h(px(184.0))
                .flex()
                .gap_4()
                .child(usage_card(app, palette))
                .child(render_network_card(app, palette, cx)),
        )
        .child(status_bar(app, palette))
        .into_any_element()
}

// ---------------------------------------------------------------- 英雄区

/// 英雄区：左侧当前出站（走哪），右侧实时速率（多快）。
/// 整卡可点进入代理页；卡内眼睛图标切换服务器明文时拦截冒泡。
fn hero(app: &PureClash, palette: Palette, cx: &mut Context<PureClash>) -> AnyElement {
    let (group_name, node_name) = current_outbound(app);
    let total = app.traffic_down_speed + app.traffic_up_speed;
    let down = app.traffic_down_speed;
    let up = app.traffic_up_speed;
    let down_ratio = if total > 0 {
        down as f32 / total as f32
    } else {
        0.0
    };
    let up_ratio = if total > 0 {
        up as f32 / total as f32
    } else {
        0.0
    };
    let (speed_num, speed_unit) = split_speed(total);

    let left = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .child(kit::label(tr("overview.current_outbound"), palette))
        .child(
            div()
                .mt_2()
                .text_size(px(text_size::HERO_TITLE))
                .font_semibold()
                .text_color(palette.text)
                .truncate()
                .child(group_name),
        )
        .child(
            // 航路点：实心点 = 当前选中，空心点 = 链路下游；纯装饰但给「路由」一个形状。
            div()
                .mt_3()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(route_dot(palette, true))
                .child(route_seg(palette))
                .child(route_dot(palette, false))
                .child(route_seg(palette))
                .child(route_dot(palette, false))
                .child(
                    div()
                        .ml_1()
                        .min_w_0()
                        .text_size(px(text_size::BODY))
                        .text_color(palette.accent)
                        .truncate()
                        .child(node_name.clone()),
                ),
        )
        .child(meta_row(app, node_name, palette, cx));

    let right = div()
        .w(px(300.0))
        .flex_none()
        .flex()
        .flex_col()
        .items_end()
        .child(kit::label(tr("overview.traffic"), palette))
        .child(
            div()
                .mt_1()
                .flex()
                .items_baseline()
                .gap_2()
                .child(
                    div()
                        .font_family("Bahnschrift")
                        .text_size(px(text_size::HERO_NUM))
                        .font_semibold()
                        .tabnum()
                        .text_color(palette.text)
                        .child(speed_num),
                )
                .child(
                    div()
                        .text_size(px(text_size::META))
                        .text_color(palette.muted)
                        .child(speed_unit),
                ),
        )
        .child(
            div()
                .mt_3()
                // 固定宽度并随 items_end 右对齐：与上面的速率数字同一条右边缘，
                // 用 w_full 会让它在 items_end 下向左溢出，压到左侧出站信息区。
                .w(px(260.0))
                .flex()
                .flex_col()
                .gap_2()
                .child(speed_bar(
                    palette,
                    down_ratio,
                    palette.success,
                    format!("↓ {}", format_speed(down)),
                ))
                .child(speed_bar(
                    palette,
                    up_ratio,
                    palette.amber,
                    format!("↑ {}", format_speed(up)),
                )),
        );

    kit::hero_card(palette)
        .id("overview-hero")
        .h(px(196.0))
        .flex_none()
        .min_w_0()
        .px(px(24.0))
        .py(px(20.0))
        .flex()
        .items_stretch()
        .cursor_pointer()
        .hover(|style| style.border_color(palette.brand_line))
        .child(left)
        .child(right)
        .on_click(cx.listener(|this, _, _, cx| this.select_page(Page::Proxies, cx)))
        .into_any_element()
}

/// 英雄区左下的说明行：运行模式；节点有接入信息时补协议与打码服务器。
/// 眼睛切换需拦截冒泡，避免触发整卡跳转。
fn meta_row(
    app: &PureClash,
    node_name: SharedString,
    palette: Palette,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    div()
        .mt_auto()
        .pt_2()
        .flex()
        .items_center()
        .gap_2()
        .min_w_0()
        .child(
            div()
                .text_size(px(text_size::HINT))
                .text_color(palette.faint)
                .child(app.mode.label()),
        )
        .children(app.node_endpoints.get(node_name.as_ref()).map(|endpoint| {
            let server_value = if app.server_visible {
                SharedString::from(format!("{}:{}", endpoint.server, endpoint.port))
            } else {
                SharedString::from("＊＊＊＊＊＊")
            };
            let eye = div()
                .id("toggle-server-visible")
                .flex()
                .items_center()
                .cursor_pointer()
                .child(icon(
                    if app.server_visible {
                        ICON_EYE_OFF
                    } else {
                        ICON_EYE
                    },
                    palette.faint,
                    14.0,
                ))
                .on_click(cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.server_visible = !this.server_visible;
                    cx.notify();
                }));
            div()
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .child(
                    div()
                        .text_size(px(text_size::HINT))
                        .text_color(palette.faint)
                        // 协议类型统一大写展示，如 VLESS。
                        .child(SharedString::from(endpoint.kind.to_uppercase())),
                )
                .child(
                    div()
                        .min_w_0()
                        .text_size(px(text_size::HINT))
                        .tabnum()
                        .text_color(palette.faint)
                        .truncate()
                        .child(server_value),
                )
                .child(eye)
        }))
        .into_any_element()
}

fn route_dot(palette: Palette, filled: bool) -> AnyElement {
    let dot = div()
        .w(px(11.0))
        .h(px(11.0))
        .rounded_full()
        .border_2()
        .border_color(palette.accent);
    if filled {
        dot.bg(palette.accent)
            .shadow(kit::glow(palette, 8.0))
            .into_any_element()
    } else {
        dot.into_any_element()
    }
}

fn route_seg(palette: Palette) -> AnyElement {
    div()
        .w(px(14.0))
        .h(px(2.0))
        .rounded_full()
        .bg(palette.accent)
        .opacity(0.55)
        .into_any_element()
}

/// 速率拆成「数值 + 单位」：大数字只放数值，单位小一号跟在基线上，
/// 这排布是英雄区的视觉锚点，合在一起会糊成一坨。
fn split_speed(bytes_per_second: u64) -> (String, &'static str) {
    const MB: f64 = 1024.0 * 1024.0;
    const KB: f64 = 1024.0;
    let value = bytes_per_second as f64;
    if value >= MB {
        (format!("{:.2}", value / MB), "MB/s")
    } else if value >= KB {
        (format!("{:.1}", value / KB), "KB/s")
    } else {
        (format!("{value:.0}"), "B/s")
    }
}

/// 单条速率条：轨道吃剩余宽度，数值右对齐；下行绿、上行琥珀。
fn speed_bar(palette: Palette, ratio: f32, color: Rgba, label: String) -> AnyElement {
    div()
        .w_full()
        .flex()
        .items_center()
        .gap_2()
        // progress 自身是 w_full；外层用 flex_1 的容器约束宽度，
        // 直接叠加 flex_1 会让两者的宽度语义互相覆盖，进度条被挤成半截。
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(kit::progress(palette, ratio, color)),
        )
        .child(
            div()
                .font_family("Bahnschrift")
                .text_size(px(text_size::META))
                .tabnum()
                .text_color(color)
                .child(label),
        )
        .into_any_element()
}

// ---------------------------------------------------------------- 状态带

/// 状态带：连接数（运行时绿色）/ 代理组 / 当前模式。
/// app 状态里没有规则总数，这里不放「规则」格——宁缺毋编。
fn stats_strip(app: &PureClash, palette: Palette) -> AnyElement {
    let running = app.mihomo_running();
    kit::card(palette)
        .h(px(80.0))
        .flex_none()
        .flex()
        .items_stretch()
        .child(stat_cell(
            tr("overview.active_connections"),
            SharedString::from(app.connections.len().to_string()),
            if running {
                palette.success
            } else {
                palette.text
            },
            Some(if running {
                tr("overview.live_stats")
            } else {
                tr("overview.core_not_running")
            }),
            palette,
        ))
        .child(v_divider(palette))
        .child(stat_cell(
            tr("overview.proxy_groups"),
            SharedString::from(app.groups.len().to_string()),
            palette.text,
            Some(tr("overview.loaded_groups")),
            palette,
        ))
        .child(v_divider(palette))
        .child(stat_cell(
            tr("overview.current_mode"),
            app.mode.label(),
            palette.text,
            None,
            palette,
        ))
        .into_any_element()
}

fn stat_cell(
    label: SharedString,
    value: SharedString,
    value_color: Rgba,
    sub: Option<SharedString>,
    palette: Palette,
) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .px(px(22.0))
        .flex()
        .flex_col()
        .justify_center()
        .gap(px(5.0))
        .child(kit::label(label, palette))
        .child(
            div()
                .font_family("Bahnschrift")
                .text_size(px(text_size::STAT_NUM))
                .font_semibold()
                .tabnum()
                .truncate()
                .text_color(value_color)
                .child(value),
        )
        .children(sub.map(|text| kit::hint(text, palette)))
        .into_any_element()
}

fn v_divider(palette: Palette) -> AnyElement {
    div()
        .w(px(1.0))
        .my_3()
        .flex_none()
        .bg(palette.border)
        .into_any_element()
}

// ---------------------------------------------------------------- 两张卡

/// 订阅用量卡：只列提供用量头的前三个订阅；机场不给数据的显示「未知」。
fn usage_card(app: &PureClash, palette: Palette) -> AnyElement {
    let profiles = &app.config.profiles;
    let total = profiles.len();
    let provided: Vec<_> = profiles
        .iter()
        .filter(|meta| meta.usage.is_some())
        .collect();
    kit::card(palette)
        .flex_1()
        .min_w_0()
        .h_full()
        .p(px(20.0))
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_baseline()
                .justify_between()
                .child(kit::title(tr("overview.usage_title"), palette))
                .child(kit::hint(
                    t!(
                        "overview.usage_hint",
                        provided = provided.len().to_string(),
                        total = total.to_string()
                    )
                    .into_owned(),
                    palette,
                )),
        )
        .child(
            div()
                .mt_3()
                .flex_1()
                .flex()
                .flex_col()
                .justify_around()
                .children(if provided.is_empty() {
                    vec![kit::hint(tr("profiles.usage_unknown"), palette).into_any_element()]
                } else {
                    provided
                        .iter()
                        .take(3)
                        .map(|meta| usage_row(meta, palette))
                        .collect()
                }),
        )
        .into_any_element()
}

fn usage_row(meta: &ProfileMeta, palette: Palette) -> AnyElement {
    let usage = meta.usage.as_ref().expect("仅在有用量时调用");
    let now = crate::profile::now_secs();
    let value = match usage.ratio() {
        Some(_) => SharedString::from(format!(
            "{} / {}",
            format_bytes(usage.used()),
            format_bytes(usage.total)
        )),
        // 不限量机场只报已用流量；与配置页的用量摘要口径一致。
        None => SharedString::from(format!("已用 {}", format_bytes(usage.used()))),
    };
    let left = match usage.days_left(now) {
        Some(days) if days <= 0 => tr("profiles.usage_expired").to_string(),
        Some(days) => t!("profiles.usage_days_left", days = days.to_string()).into_owned(),
        None => tr("overview.usage_no_limit").to_string(),
    };
    let ratio = usage.ratio().unwrap_or(0.0);
    let fill = if ratio >= 0.9 {
        palette.danger
    } else {
        palette.success
    };
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .flex()
                .items_baseline()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .text_size(px(text_size::ROW))
                        .text_color(palette.text)
                        .truncate()
                        .child(meta.name.clone()),
                )
                .child(
                    div()
                        .font_family("Bahnschrift")
                        .text_size(px(text_size::META))
                        .tabnum()
                        .text_color(palette.muted)
                        .child(value),
                ),
        )
        .child(kit::progress(palette, ratio as f32, fill))
        .child(
            div()
                .text_size(px(text_size::TINY))
                .text_color(palette.faint)
                .text_right()
                .child(left),
        )
        .into_any_element()
}

// ---------------------------------------------------------------- 状态条

/// 底部运行状态条：内核 / 控制器 / 系统代理 / TUN。
/// 原「运行状态卡」的四行信息收编于此；代理与 TUN 的开关在设置页。
fn status_bar(app: &PureClash, palette: Palette) -> AnyElement {
    let controller = app
        .baseline
        .as_ref()
        .map(|baseline| SharedString::from(baseline.controller_addr.clone()))
        .unwrap_or_else(|| SharedString::from("—"));
    let kernel_value = if app.mihomo_running() {
        SharedString::from(format!("Mihomo v{}", app.config.mihomo_version))
    } else {
        tr("overview.not_started")
    };
    let tun_value = if app.tun_switch_state() == SwitchState::Starting {
        tr("status.starting")
    } else if app.tun_running() {
        tr("overview.enabled")
    } else {
        tr("overview.disabled")
    };
    kit::card(palette)
        .h(px(72.0))
        .flex_none()
        .flex()
        .items_stretch()
        .child(status_cell(
            tr("overview.kernel"),
            kernel_value,
            None,
            palette,
        ))
        .child(v_divider(palette))
        .child(status_cell(
            tr("overview.controller"),
            controller,
            None,
            palette,
        ))
        .child(v_divider(palette))
        .child(status_cell(
            tr("status.system_proxy"),
            if app.system_proxy {
                tr("overview.enabled")
            } else {
                tr("overview.disabled")
            },
            Some(if app.system_proxy {
                palette.success
            } else {
                palette.muted
            }),
            palette,
        ))
        .child(v_divider(palette))
        .child(status_cell(
            tr("settings.tun"),
            tun_value,
            Some(if app.tun_running() {
                palette.success
            } else {
                palette.muted
            }),
            palette,
        ))
        .into_any_element()
}

fn status_cell(
    label: SharedString,
    value: SharedString,
    state: Option<Rgba>,
    palette: Palette,
) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .px(px(22.0))
        .flex()
        .flex_col()
        .justify_center()
        .gap(px(6.0))
        .child(kit::label(label, palette))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .children(state.map(|color| kit::status_dot(palette, color)))
                .child(
                    div()
                        .font_family("Bahnschrift")
                        .text_size(px(text_size::META + 2.0))
                        .tabnum()
                        .truncate()
                        .text_color(palette.text)
                        .child(value),
                ),
        )
        .into_any_element()
}

// ---------------------------------------------------------------- 公共

/// 系统代理托管状态文件：存在即代表客户端正在托管，异常退出后据此自愈。
pub(super) fn integration_error_banner(error: &str, palette: Palette) -> AnyElement {
    div()
        .p_3()
        .rounded_md()
        // 带底色的告示条：只给红字会读成调试输出，而不是界面提示。
        .bg(palette.danger_soft)
        .text_size(px(text_size::META))
        .text_color(palette.danger)
        .child(error.to_string())
        .into_any_element()
}

/// 当前出站摘要：全局模式看 GLOBAL 的选中节点，直连模式固定 DIRECT，
/// 规则模式取订阅里第一个可手动选择的主策略组。
fn current_outbound(app: &PureClash) -> (SharedString, SharedString) {
    match app.mode {
        ProxyMode::Direct => (
            tr("overview.outbound_direct"),
            tr("overview.outbound_direct"),
        ),
        ProxyMode::Global => {
            let node = app
                .groups
                .iter()
                .find(|group| group.name == "GLOBAL")
                .map(|group| group.now.clone())
                .unwrap_or_default();
            let node = if node.is_empty() {
                tr("overview.no_selection")
            } else {
                SharedString::from(node)
            };
            (SharedString::from("GLOBAL"), node)
        }
        ProxyMode::Rule => match app
            .groups
            .iter()
            .find(|group| group.selectable && group.name != "GLOBAL")
        {
            Some(group) => {
                let node = if group.now.is_empty() {
                    tr("overview.no_selection")
                } else {
                    SharedString::from(group.now.clone())
                };
                (SharedString::from(group.name.clone()), node)
            }
            None => (
                tr("overview.outbound_direct"),
                tr("overview.outbound_direct"),
            ),
        },
    }
}
