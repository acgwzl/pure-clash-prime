//! 代理页：运行模式切换、策略分组折叠列表与节点选择。

use gpui::{AnyElement, Context, SharedString, Styled, div, px};
use rust_i18n::t;

use super::*;
use crate::assets::{ICON_CHEVRON_DOWN, ICON_CHEVRON_RIGHT};
use crate::mihomo::controller::{GroupSnapshot, NodeSnapshot};
use crate::theme::{FontWeightExt, Palette, TabularNums, text_size};
use crate::ui::kit;

/// 节点全名提示独立于卡片布局，避免完整文本撑宽三列网格。
struct NodeNameTooltip {
    /// 原始节点名称，不使用界面中的省略文本。
    name: SharedString,
    /// 与触发提示的页面保持一致的主题配色。
    palette: Palette,
}

impl Render for NodeNameTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(480.0))
            .px_3()
            .py_2()
            .rounded_sm()
            .bg(self.palette.surface)
            .border_1()
            .border_color(self.palette.border)
            .shadow_md()
            .text_sm()
            .text_color(self.palette.text)
            // 提示允许换行且不截断，让超长名称也能完整阅读。
            .whitespace_normal()
            .child(self.name.clone())
    }
}

/// 由 GPUI 管理悬浮延迟和关闭时机，仅在实际显示提示时创建实体。
fn node_name_tooltip(
    name: String,
    palette: Palette,
) -> impl Fn(&mut Window, &mut App) -> gpui::AnyView {
    let name = SharedString::from(name);
    move |_, cx| {
        cx.new(|_| NodeNameTooltip {
            name: name.clone(),
            palette,
        })
        .into()
    }
}

fn group_kind_label(group: &GroupSnapshot) -> &'static str {
    if group.selectable {
        "proxy.kind_selector"
    } else {
        "proxy.kind_auto"
    }
}

pub(super) fn group_auto_expanded(groups: &[GroupSnapshot], name: &str) -> bool {
    let mut budget = PROXY_AUTO_EXPAND_NODE_BUDGET;
    for group in groups {
        if group.nodes.len() > PROXY_AUTO_COLLAPSE_NODES {
            if group.name == name {
                return false;
            }
            continue;
        }
        if group.name == name {
            return group.nodes.len() <= budget;
        }
        budget = budget.saturating_sub(group.nodes.len());
    }
    false
}

pub(super) fn render_proxies(
    app: &PureClash,
    palette: Palette,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    // 配置更新与模式切换共用互斥，避免热重载把新模式覆盖回旧值。
    let mode_available = app.mihomo_running() && !app.profile_actions_locked();
    div()
        .p_6()
        .flex()
        .flex_col()
        .gap_4()
        .when_some(app.proxy_error.as_ref(), |page, error| {
            page.child(super::overview::integration_error_banner(error, palette))
        })
        .child(
            // 工具行：模式分段在左，排序与刷新在右；工具不再包在一层大卡里。
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap_2()
                .children(
                    [ProxyMode::Rule, ProxyMode::Global, ProxyMode::Direct]
                        .into_iter()
                        .map(|mode| mode_button(mode, app, mode_available, palette, cx)),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .id("proxies-sort")
                        .h(px(30.0))
                        .px(px(15.0))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .whitespace_nowrap()
                        .cursor_pointer()
                        .bg(if app.node_sort_by_delay {
                            palette.brand_soft
                        } else {
                            palette.surface
                        })
                        .border_1()
                        .border_color(if app.node_sort_by_delay {
                            palette.brand_line
                        } else {
                            palette.border
                        })
                        .text_size(px(text_size::ROW))
                        .text_color(if app.node_sort_by_delay {
                            palette.brand
                        } else {
                            palette.muted
                        })
                        .child(tr(if app.node_sort_by_delay {
                            "proxy.sort_delay"
                        } else {
                            "proxy.sort_default"
                        }))
                        .on_click(cx.listener(|this, _, _, cx| {
                            // 只切换展示方式，保留分组展开状态、已加载数量和当前选择。
                            this.node_sort_by_delay = !this.node_sort_by_delay;
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id("proxies-refresh")
                        .h(px(30.0))
                        .px(px(15.0))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .whitespace_nowrap()
                        .map(|button| {
                            if mode_available {
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
                        .child(tr("proxy.refresh"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.mihomo_running() && !this.profile_actions_locked() {
                                this.proxy_error = None;
                                this.fetch_runtime_state(cx);
                            }
                        })),
                ),
        )
        .child(if app.groups.is_empty() {
            // 内核未运行或尚未拉到分组时展示引导空态。
            kit::card(palette)
                .p_8()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_size(px(text_size::BODY))
                        .text_color(palette.text)
                        .child(if app.proxies_loading {
                            tr("proxy.loading")
                        } else {
                            tr("proxy.empty_title")
                        }),
                )
                .child(kit::hint(tr("proxy.empty_detail"), palette))
                .into_any_element()
        } else if app.mode == ProxyMode::Direct {
            // 直连模式不经过任何代理节点，分组列表没有操作意义，展示提示。
            direct_mode_hint(palette).into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .children(app.groups.iter().enumerate().map(|(index, group)| {
                    let expanded = app.group_expanded(group);
                    let rendered = app.group_rendered_count(group);
                    proxy_group_panel(index, group, expanded, rendered, app, palette, cx)
                }))
                .into_any_element()
        })
        .into_any_element()
}

/// 直连模式提示卡：该模式下请求不经过代理节点，无需选择分组。
fn direct_mode_hint(palette: Palette) -> AnyElement {
    kit::card(palette)
        .p_8()
        .flex()
        .flex_col()
        .items_center()
        .gap_2()
        .child(
            div()
                .text_size(px(text_size::BODY))
                .text_color(palette.text)
                .child(tr("proxy.direct_title")),
        )
        .child(kit::hint(tr("proxy.direct_detail"), palette))
        .into_any_element()
}

fn mode_button(
    mode: ProxyMode,
    app: &PureClash,
    available: bool,
    palette: Palette,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    let active = mode == app.mode;
    div()
        .id(mode.label())
        .map(|button| {
            if available {
                button.cursor_pointer()
            } else {
                button.opacity(0.5)
            }
        })
        .h(px(30.0))
        .px(px(15.0))
        .rounded_full()
        .flex()
        .items_center()
        // 模式说明收进悬停提示：工具行保持一行高度，信息不丢。
        .tooltip(node_name_tooltip(mode.detail().into(), palette))
        .bg(if active {
            palette.brand_soft
        } else {
            palette.surface
        })
        .border_1()
        .border_color(if active {
            palette.brand_line
        } else {
            palette.border
        })
        .text_size(px(text_size::ROW))
        .font_medium()
        .text_color(if active { palette.brand } else { palette.muted })
        .child(mode.label())
        .on_click(cx.listener(move |this, _, _, cx| this.set_mode(mode, cx)))
        .into_any_element()
}

fn proxy_group_panel(
    group_index: usize,
    group: &GroupSnapshot,
    expanded: bool,
    rendered: usize,
    app: &PureClash,
    palette: Palette,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    let group_name = group.name.clone();
    let test_name = group.name.clone();
    let more_source = group.name.clone();
    let group_testing = group
        .nodes
        .iter()
        .any(|node| app.delay_testing.contains(&node.name));
    kit::card(palette)
        .py_3()
        .px_4()
        .child(
            div()
                .id(SharedString::from(format!("proxy-group-{group_index}")))
                .flex()
                .items_center()
                .justify_between()
                .cursor_pointer()
                // 整个标题栏（含空白、当前节点、数量和箭头）都可展开/收起。
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle_group_expanded(group_name.clone(), cx)
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(text_size::CARD_TITLE))
                                .font_semibold()
                                .text_color(palette.text)
                                .child(group.name.clone()),
                        )
                        .child(
                            div()
                                .px_2()
                                .py(px(2.0))
                                .rounded_full()
                                .bg(palette.inset)
                                .text_size(px(text_size::TINY))
                                .text_color(palette.muted)
                                .child(tr(group_kind_label(group))),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        // 整组测速：一次请求更新组内全部节点延迟。
                        .child(
                            div()
                                .id(SharedString::from(format!("proxy-test-{group_index}")))
                                .px_2()
                                .py(px(3.0))
                                .rounded_md()
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .bg(palette.inset)
                                .text_size(px(text_size::TINY))
                                .text_color(if group_testing {
                                    palette.accent
                                } else {
                                    palette.muted
                                })
                                .child(if group_testing {
                                    tr("proxy.testing")
                                } else {
                                    tr("proxy.test")
                                })
                                .on_click({
                                    let name = test_name.clone();
                                    cx.listener(move |this, _, _, cx| {
                                        // 测速是独立操作，不向标题栏冒泡触发折叠。
                                        cx.stop_propagation();
                                        this.test_group_delay(name.clone(), cx);
                                    })
                                }),
                        )
                        // 当前节点是这一行最该看的信息：品牌强调色，
                        // 与右侧「测速 / 节点数」这类次要信息拉开层级。
                        .children((!expanded && !group.now.is_empty()).then(|| {
                            div()
                                .id(SharedString::from(format!("proxy-current-{group_index}")))
                                .max_w(px(240.0))
                                .truncate()
                                .text_size(px(text_size::ROW))
                                .text_color(palette.accent)
                                .tooltip(node_name_tooltip(group.now.clone(), palette))
                                .child(group.now.clone())
                        }))
                        .child(
                            div()
                                .text_size(px(text_size::META))
                                .text_color(palette.faint)
                                .child(
                                    t!("proxy.nodes", count = group.nodes.len().to_string())
                                        .into_owned(),
                                ),
                        )
                        .child(icon(
                            if expanded {
                                ICON_CHEVRON_DOWN
                            } else {
                                ICON_CHEVRON_RIGHT
                            },
                            palette.muted,
                            14.0,
                        )),
                ),
        )
        // 折叠时不渲染节点行；展开时按页渲染普通网格，剩余节点通过
        // “显示更多”加载，单次布局量有硬上界，滚动只保留页面一层。
        .children(expanded.then(|| {
            let rendered = rendered.min(group.nodes.len());
            // 先对整个分组排序再分页，确保最快节点即使原本在后续页也能显示在首页。
            let indices = ordered_node_indices(&group.nodes, app.node_sort_by_delay, |node| {
                app.node_delay(node).flatten()
            });
            let mut list = div().mt_3().child(
                // 节点网格垫一层嵌入底：卡片浮在衬底上，层次比描边安静。
                // 所有节点共用等宽列（minmax(0, 1fr)），避免逐行 Flex 被长名称
                // 撑宽；末行不足三项时仍与上方列对齐，不拉伸剩余卡片。
                kit::inset_panel(palette).p_2().child(
                    div().grid().grid_cols(PROXY_NODE_COLUMNS).gap_2().children(
                        indices.into_iter().take(rendered).map(|node_index| {
                            proxy_node_row(
                                group_index,
                                node_index,
                                &group.nodes[node_index],
                                group.now.as_str(),
                                app,
                                palette,
                                cx,
                            )
                        }),
                    ),
                ),
            );
            if rendered < group.nodes.len() {
                let more_name = more_source.clone();
                let remaining = group.nodes.len() - rendered;
                list = list.child(
                    div()
                        .id(SharedString::from(format!("proxy-more-{group_index}")))
                        .mt_2()
                        .h(px(32.0))
                        .rounded_full()
                        .bg(palette.inset)
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .text_size(px(text_size::META))
                        .text_color(palette.muted)
                        .child(t!("proxy.show_more", count = remaining.to_string()).into_owned())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.show_more_nodes(more_name.clone(), cx)
                        })),
                );
            }
            list
        }))
        .into_any_element()
}

/// 生成展示用的原始下标，避免排序改变节点点击目标、持久化选择或缺失节点回退顺序。
/// 有效延迟升序；未知/超时置后，同延迟与未知节点均保持配置原序。
fn ordered_node_indices(
    nodes: &[NodeSnapshot],
    by_delay: bool,
    delay: impl Fn(&NodeSnapshot) -> Option<u64>,
) -> Vec<usize> {
    let mut indices: Vec<_> = (0..nodes.len()).collect();
    if by_delay {
        indices.sort_by_key(
            |&index| match delay(&nodes[index]).filter(|&value| value > 0) {
                Some(value) => (false, value),
                None => (true, 0),
            },
        );
    }
    indices
}

fn proxy_node_row(
    group_index: usize,
    node_index: usize,
    node: &NodeSnapshot,
    selected_name: &str,
    app: &PureClash,
    palette: Palette,
    cx: &mut Context<PureClash>,
) -> AnyElement {
    let selected = node.name == selected_name;
    div()
        .id(SharedString::from(format!(
            "proxy-node-{group_index}-{node_index}"
        )))
        // 卡片必须允许收缩，长名称只在内部省略，不能反过来撑大网格列。
        .min_w_0()
        .min_h(px(48.0))
        .px_3()
        .rounded_lg()
        .flex()
        .items_center()
        .gap_3()
        .cursor_pointer()
        // 选中 = 品牌软底 + 品牌细环；绿点保留「这是当前出口」的运行语义。
        .bg(if selected {
            palette.brand_soft
        } else {
            palette.surface
        })
        .border_1()
        .border_color(if selected {
            palette.brand_line
        } else {
            palette.border
        })
        .child(div().size_2().flex_none().rounded_full().bg(if selected {
            palette.success
        } else {
            palette.border
        }))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "proxy-node-name-{group_index}-{node_index}"
                        )))
                        .truncate()
                        .text_size(px(text_size::ROW))
                        .font_medium()
                        .text_color(palette.text)
                        .tooltip(node_name_tooltip(node.name.clone(), palette))
                        .child(node.name.clone()),
                )
                .child(
                    div()
                        .mt_1()
                        .truncate()
                        .text_size(px(text_size::TINY))
                        .text_color(palette.faint)
                        .child(node.kind.to_lowercase()),
                ),
        )
        .children(node_delay_badge(app, node, palette, cx))
        .on_click(cx.listener(move |this, _, _, cx| this.select_node(group_index, node_index, cx)))
        .into_any_element()
}

/// 节点延迟徽标：测速中显示省略号，有结果按延迟分色，点击单独重测该节点。
/// 无数据时不渲染，保持未测速节点的行高稳定。
fn node_delay_badge(
    app: &PureClash,
    node: &NodeSnapshot,
    palette: Palette,
    cx: &mut Context<PureClash>,
) -> Option<AnyElement> {
    let testing = app.delay_testing.contains(&node.name);
    let (label, fg, bg) = if testing {
        (tr("proxy.testing"), palette.muted, palette.inset)
    } else {
        match app.node_delay(node) {
            None => return None,
            Some(None) => (tr("proxy.timeout"), palette.danger, palette.danger_soft),
            Some(Some(delay)) => {
                let label = SharedString::from(format!("{delay} ms"));
                // 语义色四档：<200 健康，200–500 琥珀提醒，更高与超时红。
                // 金色只属于警示档——评审定下的纪律，不兼任品牌。
                let (fg, bg) = if delay < 200 {
                    (palette.success, palette.success_soft)
                } else if delay < 500 {
                    (palette.amber, palette.amber_soft)
                } else {
                    (palette.danger, palette.danger_soft)
                };
                (label, fg, bg)
            }
        }
    };
    let node_name = node.name.clone();
    Some(
        div()
            .id(SharedString::from(format!("proxy-delay-{}", node.name)))
            .px_2()
            .py(px(2.0))
            .rounded_full()
            .flex_none()
            // 中英文测速状态均保持单行，不因名称变长而换行挤高卡片。
            .whitespace_nowrap()
            .cursor_pointer()
            .bg(bg)
            .text_size(px(text_size::TINY))
            // 等宽数字：测速时整列数字宽度一致，徽章不会左右抽动。
            .tabnum()
            .text_color(fg)
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                // 点击徽标只重测该节点，不触发整行的节点选择。
                cx.stop_propagation();
                this.test_node_delay(node_name.clone(), cx);
            }))
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proxy_group(name: &str, node_count: usize) -> GroupSnapshot {
        GroupSnapshot {
            name: name.to_string(),
            now: String::new(),
            selectable: true,
            nodes: (0..node_count)
                .map(|index| NodeSnapshot {
                    name: format!("节点{index}"),
                    kind: "vless".into(),
                    delay: None,
                })
                .collect(),
        }
    }

    #[test]
    fn delay_sort_preserves_node_identity_and_default_order() {
        let mut group = proxy_group("测试", 6);
        for (node, delay) in
            group
                .nodes
                .iter_mut()
                .zip([None, Some(200), Some(50), Some(0), Some(50), Some(10)])
        {
            node.delay = delay;
        }
        let sorted = ordered_node_indices(&group.nodes, true, |node| node.delay);
        assert_eq!(sorted, [5, 2, 4, 1, 0, 3]);
        // 分页在排序之后截取，点击仍使用原始下标，不能把第一张卡误当成节点0。
        assert_eq!(group.nodes[sorted[0]].name, "节点5");
        assert_eq!(&sorted[..3], &[5, 2, 4]);
        assert_eq!(
            ordered_node_indices(&group.nodes, false, |_| None),
            [0, 1, 2, 3, 4, 5]
        );
        // 最新测速失败应覆盖历史快值，原本最快的节点随结果变化移到末尾。
        assert_eq!(
            ordered_node_indices(&group.nodes, true, |node| {
                if node.name == "节点5" {
                    None
                } else {
                    node.delay
                }
            }),
            [2, 4, 1, 0, 3, 5]
        );
    }

    #[test]

    fn groups_follow_config_order_with_global_first() {
        let mut groups = vec![
            proxy_group("美国", 2),
            proxy_group("GLOBAL", 3),
            proxy_group("自动选择", 1),
            proxy_group("香港", 2),
        ];
        let order = vec![
            "香港".to_string(),
            "美国".to_string(),
            "自动选择".to_string(),
        ];
        order_groups(&mut groups, &order);

        let names: Vec<&str> = groups.iter().map(|group| group.name.as_str()).collect();
        // GLOBAL 置顶，其余严格按配置定义顺序。
        assert_eq!(names, vec!["GLOBAL", "香港", "美国", "自动选择"]);

        // 顺序表缺失（解析失败兜底）时保持快照原序，只是 GLOBAL 仍在首位。
        let mut fallback = vec![proxy_group("美国", 1), proxy_group("GLOBAL", 1)];
        order_groups(&mut fallback, &[]);
        let names: Vec<&str> = fallback.iter().map(|group| group.name.as_str()).collect();
        assert_eq!(names, vec!["GLOBAL", "美国"]);
    }

    #[test]
    fn oversized_group_defaults_collapsed() {
        let groups = vec![proxy_group("大分组", 200), proxy_group("小分组", 5)];
        assert!(!group_auto_expanded(&groups, "大分组"));
        assert!(group_auto_expanded(&groups, "小分组"));
        // 不在列表中的组名按折叠处理。
        assert!(!group_auto_expanded(&groups, "未知分组"));
    }

    #[test]
    fn auto_expand_stops_at_node_budget() {
        // 8 个 20 节点的组，预算 120 只够前 6 个自动展开。
        let groups: Vec<GroupSnapshot> = (0..8)
            .map(|index| proxy_group(&format!("g{index}"), 20))
            .collect();
        assert!(group_auto_expanded(&groups, "g0"));
        assert!(group_auto_expanded(&groups, "g5"));
        assert!(!group_auto_expanded(&groups, "g6"));
        assert!(!group_auto_expanded(&groups, "g7"));
    }
}
