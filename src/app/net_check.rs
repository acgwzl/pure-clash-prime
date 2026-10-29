//! 网络检测:经内核出站的公网出口 IP 与本机内网 IP。

use std::time::Duration;

// 只引入 trait 以启用 .context(),不用它的名字:否则会遮蔽 gpui 的 Context 类型。
use anyhow::Context as _;
use gpui::{AnyElement, SharedString, Styled, div, px};
use serde::Deserialize;

use super::*;
use crate::assets::ICON_WINDOW_CLOSE;
use crate::theme::{FontWeightExt, Palette, TabularNums, text_size};
use crate::ui::kit;

/// 出口 IP 查询端点;返回 `ip` 与 `country_code`。
// 强制 IPv4：默认接口在有双栈时会返回 IPv6，地址过长且无法在状态面板完整展示。
const PUBLIC_IP_ENDPOINT: &str = "https://api-ipv4.ip.sb/geoip";

/// 出口 IP 查询超时;经代理绕行时留出余量,但不至于让按钮长期处于忙态。
const NET_CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// 出口 IP 查询响应;只关心地址与国家代码。
#[derive(Deserialize)]
struct GeoIp {
    ip: String,
    #[serde(default)]
    country_code: String,
    /// 运营商/组织名；同一接口在不同 IP 段上只给其中一个字段，两个都收。
    #[serde(default)]
    organization: String,
    #[serde(default)]
    isp: String,
    /// 国家名（英文），用于展示归属地。
    #[serde(default)]
    country: String,
    /// 城市名；部分地区接口不给。
    #[serde(default)]
    city: String,
}

/// 站点连通性探测目标；按常用程度排序。
const SITE_TARGETS: [(&str, &str); 5] = [
    ("GitHub", "https://github.com"),
    ("Google", "https://www.google.com/generate_204"),
    ("YouTube", "https://www.youtube.com"),
    ("Cloudflare", "http://cp.cloudflare.com/generate_204"),
    ("Telegram", "https://telegram.org"),
];

/// 一次网络检测的完整结果。
#[derive(Clone, Debug, Default)]
pub(crate) struct NetReport {
    pub(crate) ip: String,
    pub(crate) flag: String,
    /// 「日本 · Tokyo」形式；国家或城市缺失时只显示其一。
    pub(crate) location: String,
    pub(crate) isp: String,
    /// 出口查询往返耗时（毫秒），代表当前链路质量。
    pub(crate) latency_ms: u64,
    /// 各站点的延迟；None 表示不通。
    pub(crate) sites: Vec<(String, Option<u64>)>,
}

/// 取本机默认路由出口地址:UDP 连接不实际发包,只让内核选出一条路由,
/// 因此内网 IP 的获取不依赖网络可达。
pub(super) fn local_ip() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    Some(socket.local_addr().ok()?.ip().to_string())
}

/// 两位国家代码转区域指示符(国旗 emoji);非法输入返回空串。
///
/// 国旗有两百多个,逐个打包图标不现实;区域指示符由系统字体渲染,零资源成本。
pub(super) fn flag_of(country_code: &str) -> String {
    let upper = country_code.trim().to_ascii_uppercase();
    if upper.len() != 2 || !upper.chars().all(|c| c.is_ascii_uppercase()) {
        return String::new();
    }
    upper
        .chars()
        .filter_map(|c| char::from_u32(0x1F1E6 + (c as u32 - 'A' as u32)))
        .collect()
}

/// 探测单个站点是否可达，返回往返毫秒。
///
/// 拿到任何 HTTP 状态码都算「通」：不少站点对探测请求回 403/404，
/// 那只能说明网络到得了，不是链路故障。
fn probe_site(port: u16, url: &str) -> Option<u64> {
    let proxy = ureq::Proxy::new(&format!("http://127.0.0.1:{port}")).ok()?;
    let agent = ureq::AgentBuilder::new()
        .proxy(proxy)
        .timeout(std::time::Duration::from_secs(6))
        .build();
    let started = std::time::Instant::now();
    match agent.get(url).call() {
        Ok(_) => Some(started.elapsed().as_millis() as u64),
        Err(ureq::Error::Status(_, _)) => Some(started.elapsed().as_millis() as u64),
        Err(_) => None,
    }
}

/// 并发探测全部站点：串行时最坏情况要等 5 个超时，弹层会长时间空白。
fn probe_sites(port: u16) -> Vec<(String, Option<u64>)> {
    let mut results: Vec<(String, Option<u64>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = SITE_TARGETS
            .iter()
            .map(|(name, url)| {
                scope.spawn(move || {
                    let latency = probe_site(port, url);
                    ((*name).to_owned(), latency)
                })
            })
            .collect();
        handles
            .into_iter()
            .filter_map(|handle| handle.join().ok())
            .collect()
    });
    // 并发回来是无序的，按 SITE_TARGETS 的声明顺序还原。
    results.sort_by_key(|(name, _)| {
        SITE_TARGETS
            .iter()
            .position(|(target, _)| target == name)
            .unwrap_or(usize::MAX)
    });
    results
}

/// 经 mixed-port 出站做一次完整检测：出口信息 + 延迟 + 各站点连通性。
///
/// 走内核而不是直连：直连拿到的是本机真实地址，与「当前出口」的语义不符；
/// 内核未运行时宁可报错，也不静默给出误导性的结果。
fn probe(port: u16) -> anyhow::Result<NetReport> {
    let proxy = ureq::Proxy::new(&format!("http://127.0.0.1:{port}"))
        .map_err(|_| anyhow::anyhow!(t!("overview.network_proxy_invalid").into_owned()))?;
    let agent = ureq::AgentBuilder::new()
        .proxy(proxy)
        .timeout(NET_CHECK_TIMEOUT)
        .build();
    let started = std::time::Instant::now();
    let geo: GeoIp = agent
        .get(PUBLIC_IP_ENDPOINT)
        .call()
        .map_err(|_| anyhow::anyhow!(t!("overview.network_unreachable").into_owned()))?
        .into_json()
        .context("出口 IP 查询响应格式无效")?;
    let latency_ms = started.elapsed().as_millis() as u64;

    let location = match (geo.country.trim(), geo.city.trim()) {
        ("", "") => String::new(),
        (country, "") => country.to_owned(),
        ("", city) => city.to_owned(),
        (country, city) => format!("{country} · {city}"),
    };
    let isp = if geo.organization.is_empty() {
        geo.isp
    } else {
        geo.organization
    };

    Ok(NetReport {
        flag: flag_of(&geo.country_code),
        ip: geo.ip,
        location,
        isp,
        latency_ms,
        sites: probe_sites(port),
    })
}

impl PureClash {
    /// 检测内网 IP 与经内核出站的出口 IP;内网即时可得,公网走后台查询。
    ///
    /// 只由「检测」按钮触发:仓库口径是不隐式联网,Geo 数据都不自动下载,
    /// 每次进入概览页都请求第三方接口与之相悖。
    pub(super) fn check_network(&mut self, cx: &mut Context<Self>) {
        if self.net_check_loading {
            return;
        }
        self.net_check_local = local_ip();
        self.net_check_public = None;
        self.net_check_flag = String::new();
        self.public_isp = None;
        self.net_check_error = None;
        // 点检测即把详情弹层打开，结果逐项填进去。
        self.net_dialog_open = true;
        let Some(mixed_port) = self
            .baseline
            .as_ref()
            .filter(|_| self.mihomo_running())
            .map(|baseline| baseline.mixed_port)
        else {
            self.net_check_error = Some(t!("overview.network_core_unavailable").into_owned());
            cx.notify();
            return;
        };
        let generation = self.core_start_generation;
        self.net_check_loading = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { probe(mixed_port) })
                .await;
            let _ = this.update(cx, |this, cx| {
                // 查询期间内核可能被停止或替换;丢弃属于旧启动代的结果。
                if generation != this.core_start_generation {
                    return;
                }
                this.net_check_loading = false;
                match result {
                    Ok(report) => {
                        this.net_check_flag = report.flag.clone();
                        this.net_check_public = Some(report.ip.clone());
                        this.public_isp = if report.isp.is_empty() {
                            None
                        } else {
                            Some(report.isp.clone())
                        };
                        this.net_report = Some(report);
                    }
                    Err(error) => {
                        this.net_check_error = Some(concise_error(&format!("{error:#}"), 120));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 关闭网络详情弹层。
    pub(super) fn close_net_dialog(&mut self, cx: &mut Context<Self>) {
        self.net_dialog_open = false;
        cx.notify();
    }
}

/// 概览页网络检测卡片:出口 IP 与内网 IP 两项,手动触发检测。
pub(super) fn render_network_card(
    app: &PureClash,
    palette: Palette,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    // 有国旗时与地址同行展示,如「🇨🇳 117.159.9.103」。
    let public = if app.net_check_loading {
        tr("overview.network_checking")
    } else {
        match app.net_check_public.as_ref() {
            Some(ip) => {
                SharedString::from(format!("{} {}", app.net_check_flag, ip).trim().to_owned())
            }
            None => tr("overview.network_undetected"),
        }
    };
    let local = app
        .net_check_local
        .clone()
        .map(SharedString::from)
        .unwrap_or_else(|| tr("overview.network_undetected"));
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
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap_2()
                        .child(kit::title(tr("overview.network_check"), palette))
                        .child(kit::hint(tr("overview.network_check_detail"), palette)),
                )
                .child(card_button(
                    "network-check",
                    tr("overview.network_check_button"),
                    !app.net_check_loading,
                    palette,
                    cx.listener(|this, _, _, cx| this.check_network(cx)),
                )),
        )
        .child(
            // 紧凑三行：标签在左、值在右。原先两块上下堆叠的写法会把
            // 归属地与延迟挤出卡片（128px 放不下四行）。
            div()
                .mt_3()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .justify_around()
                .gap_1()
                .child(compact_row(
                    tr("overview.network_public_short"),
                    public,
                    palette,
                    true,
                ))
                // 归属地 + 延迟常驻：这是判断线路质量最常用的两个数。
                .child(compact_row(
                    tr("overview.net_location"),
                    net_detail_text(app),
                    palette,
                    false,
                ))
                .child(compact_row(
                    tr("overview.network_local_short"),
                    local,
                    palette,
                    false,
                )),
        )
        .when_some(app.net_check_error.clone(), |card, error| {
            card.child(
                div()
                    .mt_2()
                    .p_2()
                    .rounded_md()
                    .bg(palette.danger_soft)
                    .text_xs()
                    .text_color(palette.danger)
                    .child(SharedString::from(error)),
            )
        })
        .into_any_element()
}

/// 归属地 + 延迟的组合文案；没有检测结果时显示占位。
fn net_detail_text(app: &PureClash) -> SharedString {
    let Some(report) = app.net_report.as_ref() else {
        return tr("overview.network_undetected");
    };
    let location = if report.location.is_empty() {
        "—".to_owned()
    } else {
        report.location.clone()
    };
    SharedString::from(format!(
        "{} · {} {} ms",
        location,
        tr("overview.net_latency"),
        report.latency_ms
    ))
}

/// 卡片内一行：左标签右值。`emphasis` 为真时值更大更亮（用于主 IP）。
fn compact_row(
    label: SharedString,
    value: SharedString,
    palette: Palette,
    emphasis: bool,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .child(
            div()
                .flex_none()
                .text_size(px(text_size::HINT))
                .text_color(palette.faint)
                .child(label),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .tabnum()
                .text_size(px(if emphasis { 15.0 } else { 12.0 }))
                .text_color(if emphasis {
                    palette.text
                } else {
                    palette.muted
                })
                .child(value),
        )
        .into_any_element()
}

/// 单个检测项:标签在上、值在下。
/// IP 不再装进灰色小方块——它是这 张卡的主角，直接给足对比。
fn ip_block(label: SharedString, value: SharedString, palette: Palette) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(kit::label(label, palette))
        .child(
            div()
                .text_size(px(15.0))
                .font_semibold()
                .tabnum()
                .truncate()
                .text_color(palette.text)
                .child(value),
        )
        .into_any_element()
}

/// 卡片内的行内按钮;与其它页面的次要按钮保持同一视觉层级。
fn card_button(
    id: &'static str,
    label: SharedString,
    enabled: bool,
    palette: Palette,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .h(px(28.0))
        .px_3()
        .rounded_sm()
        .flex()
        .items_center()
        .justify_center()
        .bg(palette.inset)
        .border_1()
        .border_color(palette.border)
        .text_xs()
        .text_color(if enabled { palette.text } else { palette.muted })
        .opacity(if enabled { 1.0 } else { 0.5 })
        .child(label)
        .when(enabled, |button| button.cursor_pointer().on_click(handler))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_of_maps_country_codes_to_regional_indicators() {
        assert_eq!(flag_of("CN"), "🇨🇳");
        assert_eq!(flag_of(" jp "), "🇯🇵");
        // 非法输入返回空串,不产生半个国旗。
        assert_eq!(flag_of(""), "");
        assert_eq!(flag_of("C"), "");
        assert_eq!(flag_of("CHN"), "");
        assert_eq!(flag_of("C1"), "");
    }

    #[test]
    fn local_ip_returns_a_parseable_address_offline() {
        // 只做形态校验:UDP connect 不发包,有无外网都应能取到地址。
        let value = local_ip().expect("本机应能选出默认路由出口地址");
        assert!(
            value.parse::<std::net::IpAddr>().is_ok(),
            "取到的应是可解析的 IP:{value}"
        );
    }
}

/// 网络详情弹层：出口信息（归属地/运营商/延迟）+ 各站点连通性。
///
/// 统一放在这里，避免把这些信息摊到四张信息卡里挤成一团。
pub(super) fn render_net_dialog(
    app: &PureClash,
    palette: Palette,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    let report = app.net_report.clone();
    let local = app
        .net_check_local
        .clone()
        .unwrap_or_else(|| "—".to_owned());

    let mut rows: Vec<AnyElement> = Vec::new();
    if let Some(report) = report.as_ref() {
        let ip_line = if report.flag.is_empty() {
            report.ip.clone()
        } else {
            format!("{} {}", report.flag, report.ip)
        };
        rows.push(detail_row(tr("overview.network_public"), ip_line, palette));
        rows.push(detail_row(
            tr("overview.net_location"),
            if report.location.is_empty() {
                "—".to_owned()
            } else {
                report.location.clone()
            },
            palette,
        ));
        rows.push(detail_row(
            tr("overview.net_isp"),
            if report.isp.is_empty() {
                "—".to_owned()
            } else {
                report.isp.clone()
            },
            palette,
        ));
        rows.push(detail_row(
            tr("overview.net_latency"),
            format!("{} ms", report.latency_ms),
            palette,
        ));
        rows.push(detail_row(tr("overview.network_local"), local, palette));
    }

    let site_rows: Vec<AnyElement> = report
        .as_ref()
        .map(|report| {
            report
                .sites
                .iter()
                .map(|(name, latency)| {
                    let (text, color) = match latency {
                        Some(ms) => (format!("{ms} ms"), latency_color(*ms, palette)),
                        None => (tr("overview.net_timeout").to_string(), palette.danger),
                    };
                    div()
                        .py(px(6.0))
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(text_size::ROW))
                                .text_color(palette.text)
                                .child(SharedString::from(name.clone())),
                        )
                        .child(
                            div()
                                .font_family("Bahnschrift")
                                .tabnum()
                                .text_size(px(text_size::META))
                                .text_color(color)
                                .child(SharedString::from(text)),
                        )
                        .into_any_element()
                })
                .collect()
        })
        .unwrap_or_default();

    div()
        .id("net-dialog-scrim")
        .absolute()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui::rgba(0x00000099))
        .on_click(cx.listener(|this, _, _, cx| this.close_net_dialog(cx)))
        .child(
            div()
                .id("net-dialog-panel")
                .w(px(420.0))
                .flex()
                .flex_col()
                .rounded_lg()
                .bg(palette.surface_high)
                .border_1()
                .border_color(palette.border)
                .shadow_lg()
                .on_click(cx.listener(|_, _, _, cx| cx.stop_propagation()))
                .child(
                    div()
                        .px_4()
                        .py_3()
                        .border_b_1()
                        .border_color(palette.border)
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(text_size::BODY))
                                .font_semibold()
                                .text_color(palette.text)
                                .child(tr("overview.net_detail_title")),
                        )
                        .child(
                            div()
                                .id("net-dialog-close")
                                .cursor_pointer()
                                .hover(|style| style.opacity(0.7))
                                .child(icon(ICON_WINDOW_CLOSE, palette.muted, 14.0))
                                .on_click(cx.listener(|this, _, _, cx| this.close_net_dialog(cx))),
                        ),
                )
                .when(app.net_check_loading, |panel| {
                    panel.child(
                        div()
                            .p_4()
                            .text_size(px(text_size::META))
                            .text_color(palette.muted)
                            .child(tr("overview.net_checking")),
                    )
                })
                .children(rows)
                .when_some(app.net_check_error.clone(), |panel, error| {
                    panel.child(
                        div()
                            .mx_4()
                            .mb_3()
                            .p_2()
                            .rounded_md()
                            .bg(palette.danger_soft)
                            .text_size(px(text_size::HINT))
                            .text_color(palette.danger)
                            .child(SharedString::from(error)),
                    )
                })
                .when(!site_rows.is_empty(), |panel| {
                    panel
                        .child(
                            div()
                                .px_4()
                                .pt_3()
                                .pb_1()
                                .text_size(px(text_size::HINT))
                                .text_color(palette.faint)
                                .child(tr("overview.net_sites")),
                        )
                        .child(div().px_4().pb_3().flex().flex_col().children(site_rows))
                }),
        )
        .into_any_element()
}

/// 详情里的一行：左标签右值。
fn detail_row(label: SharedString, value: String, palette: Palette) -> AnyElement {
    div()
        .px_4()
        .py(px(5.0))
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .child(
            div()
                .flex_none()
                .text_size(px(text_size::HINT))
                .text_color(palette.faint)
                .child(label),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .font_family("Bahnschrift")
                .tabnum()
                .text_size(px(text_size::ROW))
                .text_color(palette.text)
                .child(SharedString::from(value)),
        )
        .into_any_element()
}

/// 延迟着色：200ms 内算好，800ms 内一般，再慢标红。
fn latency_color(ms: u64, palette: Palette) -> gpui::Rgba {
    if ms <= 200 {
        palette.success
    } else if ms <= 800 {
        palette.amber
    } else {
        palette.danger
    }
}
