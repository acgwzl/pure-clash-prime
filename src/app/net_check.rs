//! 网络检测:经内核出站的公网出口 IP 与本机内网 IP。

use std::time::Duration;

// 只引入 trait 以启用 .context(),不用它的名字:否则会遮蔽 gpui 的 Context 类型。
use anyhow::Context as _;
use gpui::{AnyElement, SharedString, Styled, div, px};
use serde::Deserialize;

use super::*;
use crate::theme::{FontWeightExt, Palette, TabularNums};
use crate::ui::kit;

/// 出口 IP 查询端点;返回 `ip` 与 `country_code`。
const PUBLIC_IP_ENDPOINT: &str = "https://api.ip.sb/geoip";

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

/// 经 mixed-port 出站查询出口 IP。
///
/// 走内核而不是直连:直连拿到的是本机真实地址,与「当前出口」的语义不符;
/// 内核未运行时宁可报错,也不静默给出误导性的结果。
fn lookup_public(mixed_port: u16) -> anyhow::Result<(String, String, String)> {
    let proxy = ureq::Proxy::new(&format!("http://127.0.0.1:{mixed_port}"))
        .map_err(|_| anyhow::anyhow!(t!("overview.network_proxy_invalid").into_owned()))?;
    let agent = ureq::AgentBuilder::new()
        .proxy(proxy)
        .timeout(NET_CHECK_TIMEOUT)
        .build();
    let geo: GeoIp = agent
        .get(PUBLIC_IP_ENDPOINT)
        .call()
        .map_err(|_| anyhow::anyhow!(t!("overview.network_unreachable").into_owned()))?
        .into_json()
        .context("出口 IP 查询响应格式无效")?;
    let isp = if geo.organization.is_empty() {
        geo.isp
    } else {
        geo.organization
    };
    Ok((geo.ip, geo.country_code, isp))
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
                .spawn(async move { lookup_public(mixed_port) })
                .await;
            let _ = this.update(cx, |this, cx| {
                // 查询期间内核可能被停止或替换;丢弃属于旧启动代的结果。
                if generation != this.core_start_generation {
                    return;
                }
                this.net_check_loading = false;
                match result {
                    Ok((ip, country_code, isp)) => {
                        this.net_check_flag = flag_of(&country_code);
                        this.net_check_public = Some(ip);
                        this.public_isp = if isp.is_empty() { None } else { Some(isp) };
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
            div()
                .mt_3()
                .flex_1()
                .flex()
                .flex_col()
                .justify_around()
                .gap_2()
                .child(ip_block(tr("overview.network_public"), public, palette))
                .child(ip_block(tr("overview.network_local"), local, palette)),
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
