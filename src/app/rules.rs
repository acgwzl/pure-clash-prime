//! 规则页：内核规则列表、匹配顺序与实时命中统计。
//!
//! 规则数组顺序即内核匹配顺序，越靠前优先级越高，因此列表按序号展示，
//! 排障时可以直接看出是哪一条规则先把流量截走。
//!
//! 「命中」列来自连接页同一份快照：内核为每条连接返回命中的规则类型与内容，
//! 这里把它与规则列表对齐，因此命中统计不需要额外请求，随连接轮询实时刷新。

use std::collections::HashMap;

use gpui::{AnyElement, Context, Styled, div, px};
use rust_i18n::t;

use super::*;
use crate::assets::ICON_REFRESH_CW;
use crate::mihomo::controller::{ConnectionItem, RuleItem};
use crate::theme::{Palette, TabularNums, text_size};

/// 规则列表单页渲染的行数上限。大订阅的规则可达数千条，分页让单次布局量有硬上界。
pub(super) const RULES_PAGE_SIZE: usize = 200;

impl PureClash {
    /// 规则页按页递增渲染行数；步长与单页上限一致。
    pub(super) fn show_more_rules(&mut self, cx: &mut Context<Self>) {
        self.rules_shown = self.rules_shown.saturating_add(RULES_PAGE_SIZE);
        cx.notify();
    }
}

pub(super) fn render_rules(
    app: &PureClash,
    palette: Palette,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    div()
        .p_6()
        .flex()
        .flex_col()
        .gap_4()
        .child(
            // 工具行平铺：计数说明在左，刷新在右；表格直接落在页面底上，
            // 不再包一层大卡——表格本身就是这张页面的主体。
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap_3()
                .when(!app.rules.is_empty(), |row| {
                    row.child(
                        div()
                            .text_size(px(text_size::META))
                            .text_color(palette.faint)
                            .child(SharedString::from(
                                t!("rules.count", count = app.rules.len().to_string()).into_owned(),
                            )),
                    )
                })
                .child(div().flex_1())
                .child(refresh_button(app, palette, cx)),
        )
        .child(rules_body(app, palette, cx))
        .into_any_element()
}

/// 刷新按钮：内核未运行或正在拉取时禁用，避免重复请求。
fn refresh_button(app: &PureClash, palette: Palette, cx: &mut Context<PureClash>) -> AnyElement {
    let available = app.mihomo_running() && !app.rules_loading;
    div()
        .id("rules-refresh")
        .px(px(15.0))
        .h(px(30.0))
        .rounded_full()
        .flex()
        .items_center()
        .gap_2()
        .map(|button| {
            if available {
                button.cursor_pointer()
            } else {
                button.opacity(0.5)
            }
        })
        .bg(palette.surface)
        .border_1()
        .border_color(palette.border)
        .text_size(px(text_size::ROW))
        .text_color(palette.muted)
        .child(icon(ICON_REFRESH_CW, palette.muted, 13.0))
        .child(if app.rules_loading {
            tr("rules.loading")
        } else {
            tr("rules.refresh")
        })
        .on_click(cx.listener(|this, _, _, cx| {
            if this.mihomo_running() && !this.rules_loading {
                this.fetch_rules(cx);
            }
        }))
        .into_any_element()
}

fn rules_body(app: &PureClash, palette: Palette, cx: &mut Context<PureClash>) -> AnyElement {
    // 三处提示必须各说各的：内核没运行时让人去启动内核是对的，
    // 但内核已经在运行却还这么说，就是在让用户做一件已经做完的事。
    if !app.mihomo_running() {
        return state_block(tr("rules.empty_no_core"), tr("rules.empty_hint"), palette);
    }
    if let Some(error) = app.rules_error.as_ref() {
        return state_block(
            SharedString::from(t!("rules.error", error = concise_error(error, 120)).into_owned()),
            tr("rules.error_hint"),
            palette,
        );
    }
    if app.rules.is_empty() {
        return state_block(
            if app.rules_loading {
                tr("rules.loading")
            } else {
                tr("rules.empty")
            },
            if app.rules_loading {
                tr("rules.loading_hint")
            } else {
                tr("rules.empty_running_hint")
            },
            palette,
        );
    }

    // 命中统计每帧重建：先按连接聚合，再逐条规则查表，整体是
    // O(连接数 + 规则数)；规则可有数千条，双重循环会拖慢每帧布局。
    let hits = hit_counts(&app.connections);
    let shown = app.rules_shown.min(app.rules.len());
    let mut rows: Vec<AnyElement> = app
        .rules
        .iter()
        .take(shown)
        .enumerate()
        .map(|(index, rule)| {
            let active_hits = hits.get(&rule.key()).copied().unwrap_or(0);
            rule_row(index, rule, active_hits, palette)
        })
        .collect();
    let remaining = app.rules.len() - shown;
    if remaining > 0 {
        rows.push(show_more_button(remaining, palette, cx));
    }

    div()
        .child(rules_header(palette))
        .children(rows)
        .into_any_element()
}

/// 表头列宽必须与 `rule_row` 完全一致，否则长内容会挤动列。
fn rules_header(palette: Palette) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_3()
        .px_3()
        .h(px(34.0))
        .rounded_lg()
        .bg(palette.inset)
        .text_size(px(text_size::HINT))
        .text_color(palette.muted)
        .child(div().w(px(48.0)).child(tr("rules.header_index")))
        .child(div().flex_1().child(tr("rules.header_rule")))
        .child(div().w(px(120.0)).child(tr("rules.header_proxy")))
        .child(
            div()
                .w(px(88.0))
                .text_right()
                .child(tr("rules.header_hits")),
        )
        .child(
            div()
                .w(px(72.0))
                .text_right()
                .child(tr("rules.header_active")),
        )
        .into_any_element()
}

fn rule_row(index: usize, rule: &RuleItem, active_hits: usize, palette: Palette) -> AnyElement {
    // 累计命中是内核统计的历史总量，用普通文字色；活跃数代表此刻的连接，
    // 才值得用强调色高亮，排障时一眼看出流量正走在哪几条规则上。
    let active_color = if active_hits > 0 {
        palette.accent
    } else {
        palette.muted
    };
    let total_hits = rule.extra.hit_count;
    let total_color = if total_hits > 0 {
        palette.text
    } else {
        palette.muted
    };
    let proxy = if rule.proxy.is_empty() {
        "—".to_owned()
    } else {
        rule.proxy.clone()
    };
    let row = div()
        .min_h(px(36.0))
        .px_3()
        .flex()
        .items_center()
        .gap_3()
        .border_b_1()
        .border_color(palette.border)
        .text_xs()
        .child(
            div()
                .w(px(48.0))
                .text_color(palette.faint)
                .child(SharedString::from(rule.display_index(index).to_string())),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(palette.text)
                .text_ellipsis()
                .whitespace_nowrap()
                .child(SharedString::from(rule.rule_label())),
        )
        .child(
            div()
                .w(px(120.0))
                .text_ellipsis()
                .whitespace_nowrap()
                .text_color(palette.muted)
                .child(SharedString::from(proxy)),
        )
        // 命中数是这个表最需要横向比较的一列：右对齐 + 等宽，
        // 位数才能对齐，一眼看出哪条规则命中最多。
        .child(
            div()
                .w(px(88.0))
                .text_right()
                .tabnum()
                .text_color(total_color)
                .child(SharedString::from(if total_hits > 0 {
                    total_hits.to_string()
                } else {
                    "—".to_owned()
                })),
        )
        .child(
            div()
                .w(px(72.0))
                .text_right()
                .tabnum()
                .text_color(active_color)
                .child(SharedString::from(if active_hits > 0 {
                    active_hits.to_string()
                } else {
                    "—".to_owned()
                })),
        );
    // 有活跃命中的行整行铺品牌软底并加左侧品牌条：排障时一眼看出
    // 流量正走在哪几条规则上——这是这张表唯一的强调，别的行都保持安静。
    if active_hits > 0 {
        row.bg(palette.brand_soft)
            .relative()
            .child(
                div()
                    .absolute()
                    .left(px(0.))
                    .top(px(5.))
                    .bottom(px(5.))
                    .w(px(3.))
                    .rounded_full()
                    .bg(palette.brand),
            )
            .into_any_element()
    } else {
        row.into_any_element()
    }
}

fn show_more_button(remaining: usize, palette: Palette, cx: &mut Context<PureClash>) -> AnyElement {
    div()
        .id("rules-show-more")
        .mt_3()
        .h(px(32.0))
        .rounded_full()
        .bg(palette.inset)
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_size(px(text_size::META))
        .text_color(palette.muted)
        .child(SharedString::from(
            t!("rules.more", count = remaining.to_string()).into_owned(),
        ))
        .on_click(cx.listener(|this, _, _, cx| this.show_more_rules(cx)))
        .into_any_element()
}

/// 空态、加载态与错误态共用的引导块。
pub(super) fn state_block(title: SharedString, hint: SharedString, palette: Palette) -> AnyElement {
    div()
        .py(px(36.0))
        .flex()
        .flex_col()
        .items_center()
        .gap_2()
        .child(div().text_sm().text_color(palette.text).child(title))
        .child(div().text_xs().text_color(palette.muted).child(hint))
        .into_any_element()
}

/// 统计活跃连接对各规则的命中次数。
///
/// 键 `(规则类型, 规则内容)` 与 `RuleItem::key` 一致。规则为空白的连接
/// （例如内核自身的直连探测）不代表任何规则命中，直接跳过。
fn hit_counts(connections: &[ConnectionItem]) -> HashMap<(&str, &str), usize> {
    let mut counts: HashMap<(&str, &str), usize> = HashMap::new();
    for connection in connections {
        if connection.rule.is_empty() {
            continue;
        }
        *counts
            .entry((connection.rule.as_str(), connection.rule_payload.as_str()))
            .or_insert(0) += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 命中统计只关心规则字段，其余字段保持最小。
    fn connection(rule: &str, payload: &str) -> ConnectionItem {
        serde_json::from_str(&format!(
            r#"{{"rule": "{rule}", "rulePayload": "{payload}"}}"#
        ))
        .unwrap()
    }

    #[test]
    fn hit_counts_aggregates_by_rule_identity() {
        let connections = vec![
            connection("DomainSuffix", "example.com"),
            connection("DomainSuffix", "example.com"),
            connection("GeoIP", "CN"),
        ];
        let counts = hit_counts(&connections);
        assert_eq!(counts.get(&("DomainSuffix", "example.com")), Some(&2));
        assert_eq!(counts.get(&("GeoIP", "CN")), Some(&1));
        assert_eq!(counts.get(&("MATCH", "")), None);
    }

    #[test]
    fn hit_counts_skips_connections_without_rule() {
        // 内核自身的直连探测没有命中规则，不能计入任何一行。
        assert!(hit_counts(&[connection("", "")]).is_empty());
    }
}
