//! 页面容器：路由到各页并承载滚动区。
//!
//! 页名与副标题由顶栏承载（见 frame.rs），系统代理 / TUN 快捷开关与
//! 内核启停也已上移顶栏——这里不再渲染页头，避免同屏两份「我在哪」。

#[cfg(target_os = "linux")]
use gpui::Decorations;
use gpui::{AnyElement, Context, Styled, Window, div};

use super::*;

use super::about::render_about;
use super::connections::render_connections;
use super::extra_rules::render_extra_rules;
use super::overview::render_overview;
use super::profiles::render_profiles;
use super::proxies::render_proxies;
use super::rules::render_rules;
use super::settings::render_settings;

pub(super) fn render_page(
    app: &PureClash,
    palette: Palette,
    window: &mut Window,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    let page = div()
        .flex_1()
        .min_w_0()
        .h_full()
        .flex()
        .flex_col()
        .bg(palette.background)
        .child(
            div()
                .id("page-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(match app.page {
                    Page::Overview => render_overview(app, palette, cx),
                    Page::Proxies => render_proxies(app, palette, cx),
                    Page::Rules => render_rules(app, palette, cx),
                    Page::ExtraRules => render_extra_rules(app, palette, cx),
                    Page::Connections => render_connections(app, palette, cx),
                    Page::Profiles => render_profiles(app, palette, cx),
                    Page::Settings => render_settings(app, palette, cx),
                    Page::About => render_about(app, palette, cx),
                }),
        );

    #[cfg(target_os = "linux")]
    let page = match window.window_decorations() {
        Decorations::Client { tiling } if !tiling.bottom && !tiling.right => {
            page.rounded_br(crate::platform::CLIENT_SIDE_DECORATION_SIZE)
        }
        _ => page,
    };

    #[cfg(not(target_os = "linux"))]
    let _ = window;

    page.into_any_element()
}
