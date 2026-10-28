//! 视图状态：页面切换、调色板、策略组展开分页、顶层渲染。
//!
//! 重建：源码在 rewind 快照中丢失，方法实现取自上游 app/mod.rs 的同名方法，
//! 随后按本仓库的字段与调用点修正。

#![allow(unused_imports, dead_code)]

use super::*;

impl PureClash {

    pub(super) fn palette(&self) -> Palette {
        if self.config.theme.is_dark() {
            Palette::dark()
        } else {
            Palette::light()
        }
    }

    pub(super) fn select_page(&mut self, page: Page, cx: &mut Context<Self>) {
        if page == Page::Settings {
            self.refresh_autostart_status();
        }
        self.page = page;
        // 规则列表来自内核快照，进入页面时才拉取；已拉过则复用。
        if page == Page::Rules && self.rules.is_empty() && !self.rules_loading && self.mihomo_running()
        {
            self.fetch_rules(cx);
        }
        cx.notify();
    }

    /// 分组当前是否展开：用户手动切换优先，未手动操作过的组走自动折叠规则。
    pub(super) fn group_expanded(&self, group: &GroupSnapshot) -> bool {
        self.group_expanded
            .get(&group.name)
            .copied()
            .unwrap_or_else(|| group_auto_expanded(&self.groups, &group.name))
    }

    /// 点击分组标题切换折叠；手动状态跨刷新保留，切换配置后按组名继续生效。
    pub(super) fn toggle_group_expanded(&mut self, name: String, cx: &mut Context<Self>) {
        if let Some(group) = self.groups.iter().find(|group| group.name == name) {
            let expanded = !self.group_expanded(group);
            self.group_expanded.insert(name, expanded);
            cx.notify();
        }
    }

    /// 分组当前渲染的节点数：默认首页，点过“显示更多”后按页累加。
    pub(super) fn group_rendered_count(&self, group: &GroupSnapshot) -> usize {
        self.group_page
            .get(&group.name)
            .copied()
            .map_or(PROXY_NODE_PAGE_SIZE, |count| count.min(group.nodes.len()))
    }

    /// “显示更多”翻页：在当前渲染数上加一页，封顶到节点总数。
    pub(super) fn show_more_nodes(&mut self, name: String, cx: &mut Context<Self>) {
        if let Some(group) = self.groups.iter().find(|group| group.name == name) {
            let next =
                (self.group_rendered_count(group) + PROXY_NODE_PAGE_SIZE).min(group.nodes.len());
            self.group_page.insert(name, next);
            cx.notify();
        }
    }

}
