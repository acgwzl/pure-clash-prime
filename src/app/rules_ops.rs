//! 全局附加规则的操作：添加、删除、启停与配置事务。
//!
//! 事务链与「强制进程名」开关完全一致：
//! 1. `app.json` 先行持久化，它是开关与规则内容的唯一事实来源；
//! 2. 基线与 runtime 走同一次 `-t` 校验后原子落盘；
//! 3. 运行中的内核经 controller 热重载，不重启进程、不断开存量连接；
//! 4. 任一步失败都回滚内存、app.json、local.yaml 与 runtime.yaml 四处状态。
//!
//! 只有「已启用」的规则进入基线：停用的规则仍留在 app.json 里，随时可以再开启。

use super::*;
use crate::app::extra_rules::OPTION_PAGE_SIZE;
use crate::config::ExtraRule;
use crate::mihomo::config::{LocalBaseline, validate_rule_text};
use uuid::Uuid;

/// 附加规则支持的类型；顺序按使用频率排列，智能识别与界面列表共用同一份。
///
/// 不追求覆盖内核的全部类型（版本间仍会增加），但覆盖日常手动加规则会遇到
/// 的绝大多数；类型与出口的最终合法性仍由同版本内核 `-t` 终审。
pub(super) const RULE_TYPES: [&str; 18] = [
    "DOMAIN",
    "DOMAIN-SUFFIX",
    "DOMAIN-KEYWORD",
    "DOMAIN-REGEX",
    "DOMAIN-WILDCARD",
    "GEOSITE",
    "GEOIP",
    "IP-CIDR",
    "IP-CIDR6",
    "IP-SUFFIX",
    "IP-ASN",
    "SRC-IP-CIDR",
    "SRC-PORT",
    "DST-PORT",
    "PROCESS-NAME",
    "RULE-SET",
    "NETWORK",
    "MATCH",
];

/// 类型名是否已知（大小写不敏感）；用于区分三段完整规则与「内容,目标」。
fn is_known_rule_type(value: &str) -> bool {
    RULE_TYPES
        .iter()
        .any(|name| name.eq_ignore_ascii_case(value))
}

/// 把类型名规范成表内写法；不在表内时原样返回。
fn canonical_rule_type(value: &str) -> String {
    RULE_TYPES
        .iter()
        .find(|name| name.eq_ignore_ascii_case(value))
        .map(|name| (*name).to_owned())
        .unwrap_or_else(|| value.to_owned())
}

/// `addr/mask` 形态且掩码是纯数字时视为 CIDR。
///
/// 必须先于 host_of_url 判断：路径分隔符与掩码都是斜杠，先做 URL 提取会把
/// `1.2.3.4/24` 切成 `1.2.3.4`，静默丢掉掩码。
fn looks_like_cidr(value: &str) -> bool {
    value.split_once('/').is_some_and(|(addr, mask)| {
        !addr.is_empty() && !mask.is_empty() && mask.chars().all(|c| c.is_ascii_digit())
    })
}

/// 去掉主机名尾部端口；IPv6 字面量（方括号或含多个冒号）原样保留。
fn strip_port(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    if host.matches(':').count() == 1 {
        return host.split(':').next().unwrap_or(host);
    }
    host
}

/// 按内容形状推断规则类型，结果一定落在 `RULE_TYPES` 内。
fn guess_rule_type(payload: &str) -> String {
    let cidr = looks_like_cidr(payload);
    if cidr && payload.contains(':') {
        return "IP-CIDR6".to_owned();
    }
    if cidr {
        return "IP-CIDR".to_owned();
    }
    if !payload.is_empty() && payload.chars().all(|c| c.is_ascii_digit()) {
        return "DST-PORT".to_owned();
    }
    if payload.contains('.') && !payload.contains(' ') {
        return "DOMAIN-SUFFIX".to_owned();
    }
    "DOMAIN-KEYWORD".to_owned()
}

/// 从自由文本识别一条规则，返回 `(类型, 内容, 目标)`。
///
/// 支持四类输入，覆盖「从浏览器地址栏复制」「只记得域名」「粘贴整条规则」：
/// - `类型,内容,目标`：首段是已知类型时按三段拆分；
/// - `内容,目标`：首段不是已知类型时按内容推断类型；
/// - `内容`：仅内容，目标留空由界面补选；
/// - URL 或 `主机:端口`：取主机部分作为内容（`https://a.example.com/x?y=1` → `a.example.com`）。
///
/// 四段及以上（例如带 `no-resolve`）返回 `None`：结构化模型只存三段，
/// 静默丢弃附加参数会让规则行为与用户预期不符。
pub(super) fn recognize_rule_text(input: &str) -> Option<(String, String, String)> {
    let text = input.trim();
    if text.is_empty() {
        return None;
    }
    let fields: Vec<&str> = text.split(',').map(str::trim).collect();
    match fields.as_slice() {
        [only] => {
            let payload = if looks_like_cidr(only) {
                (*only).to_owned()
            } else if let Some(host) = strip_host(only) {
                host
            } else {
                (*only).to_owned()
            };
            Some((guess_rule_type(&payload), payload, String::new()))
        }
        [first, second] => {
            if is_known_rule_type(first) {
                Some((
                    canonical_rule_type(first),
                    String::new(),
                    (*second).to_owned(),
                ))
            } else {
                let payload = strip_host(first).unwrap_or_else(|| (*first).to_owned());
                Some((guess_rule_type(&payload), payload, (*second).to_owned()))
            }
        }
        [kind, payload, target] => {
            if !is_known_rule_type(kind) {
                return None;
            }
            let payload = if looks_like_cidr(payload) {
                (*payload).to_owned()
            } else {
                strip_host(payload).unwrap_or_else(|| (*payload).to_owned())
            };
            Some((canonical_rule_type(kind), payload, (*target).to_owned()))
        }
        _ => None,
    }
}

/// 输入看起来带协议、路径或端口时取主机名；纯主机名返回 `None` 表示无需改写。
///
/// 复用订阅日志的 `host_of_url`：它已经处理了协议、userinfo、路径、查询与片段。
fn strip_host(value: &str) -> Option<String> {
    let host = strip_port(host_of_url(value));
    if host.is_empty() || host == value {
        return None;
    }
    Some(host.to_owned())
}

/// 取出参与运行时合并的规则文本；停用的规则不进入内核配置。
pub(super) fn enabled_extra_rule_texts(rules: &[ExtraRule]) -> Vec<String> {
    rules
        .iter()
        .filter(|rule| rule.enabled)
        .map(ExtraRule::to_rule_text)
        .collect()
}

/// 可选出口：内置动作 + 当前配置里的代理组名。
///
/// 组名取自 runtime.yaml 的分组定义，因此内核未运行时也能选到真实组名；
/// 与激活哪个订阅无关，切换配置后列表自动跟着变。
pub(super) fn extra_rule_targets(app: &PureClash) -> Vec<String> {
    let mut targets = vec![
        "DIRECT".to_owned(),
        "REJECT".to_owned(),
        "REJECT-DROP".to_owned(),
    ];
    for name in read_proxy_group_order(&app.paths.runtime_mihomo_config_file) {
        if !name.is_empty() && !targets.contains(&name) {
            targets.push(name);
        }
    }
    targets
}

impl PureClash {
    /// 打开编辑器；类型与出口带默认值，减少必填项。
    pub(super) fn open_extra_rule_editor(&mut self, cx: &mut Context<Self>) {
        self.extra_rule_editor_open = true;
        self.extra_rule_type_picker = false;
        self.extra_rule_target_picker = false;
        self.extra_rule_options_shown = OPTION_PAGE_SIZE;
        self.extra_rule_error = None;
        self.extra_rule_notice = None;
        if self.extra_rule_type.is_empty() {
            self.extra_rule_type = RULE_TYPES[0].to_owned();
        }
        let targets = extra_rule_targets(self);
        if !targets.contains(&self.extra_rule_target) {
            self.extra_rule_target = targets.first().cloned().unwrap_or_default();
        }
        self.extra_rule_payload
            .update(cx, |input, cx| input.set_content(String::new(), cx));
        cx.notify();
    }

    /// 关闭编辑器并收起选择列表。
    pub(super) fn close_extra_rule_editor(&mut self, cx: &mut Context<Self>) {
        self.extra_rule_editor_open = false;
        self.extra_rule_type_picker = false;
        self.extra_rule_target_picker = false;
        self.extra_rule_error = None;
        self.extra_rule_notice = None;
        cx.notify();
    }

    /// 展开或收起规则类型列表；每次展开都从第一页开始。
    pub(super) fn toggle_extra_rule_type_picker(&mut self, cx: &mut Context<Self>) {
        self.extra_rule_type_picker = !self.extra_rule_type_picker;
        if self.extra_rule_type_picker {
            self.extra_rule_options_shown = OPTION_PAGE_SIZE;
        }
        self.extra_rule_target_picker = false;
        cx.notify();
    }

    /// 选择一个规则类型。
    pub(super) fn select_extra_rule_type(&mut self, rule_type: String, cx: &mut Context<Self>) {
        self.extra_rule_type = rule_type;
        self.extra_rule_type_picker = false;
        cx.notify();
    }

    /// 展开或收起出口列表；每次展开都从第一页开始。
    pub(super) fn toggle_extra_rule_target_picker(&mut self, cx: &mut Context<Self>) {
        self.extra_rule_target_picker = !self.extra_rule_target_picker;
        if self.extra_rule_target_picker {
            self.extra_rule_options_shown = OPTION_PAGE_SIZE;
        }
        self.extra_rule_type_picker = false;
        cx.notify();
    }

    /// 选择一个出口。
    pub(super) fn select_extra_rule_target(&mut self, target: String, cx: &mut Context<Self>) {
        self.extra_rule_target = target;
        self.extra_rule_target_picker = false;
        cx.notify();
    }

    /// 收起选择页，回到编辑状态。
    pub(super) fn close_extra_rule_picker(&mut self, cx: &mut Context<Self>) {
        self.extra_rule_type_picker = false;
        self.extra_rule_target_picker = false;
        cx.notify();
    }

    /// 选择页按页展开更多选项；选择页是页面唯一内容，不存在嵌套滚动。
    pub(super) fn show_more_extra_rule_options(&mut self, cx: &mut Context<Self>) {
        self.extra_rule_options_shown = self
            .extra_rule_options_shown
            .saturating_add(OPTION_PAGE_SIZE);
        cx.notify();
    }

    /// 识别当前输入并回填类型与目标，让编辑器显示的就是将要提交的内容。
    ///
    /// 输入自带目标时以输入为准；否则保留界面上选中的目标。
    pub(super) fn sync_extra_rule_draft(&mut self, cx: &mut Context<Self>) {
        let raw = self.extra_rule_payload.read(cx).content().trim().to_owned();
        if let Some((rule_type, _payload, target)) = recognize_rule_text(&raw) {
            self.extra_rule_type = rule_type;
            if !target.is_empty() {
                self.extra_rule_target = target;
            }
        }
        cx.notify();
    }

    /// 提交编辑器里的规则：类型与出口必填，内容按类型可选。
    pub(super) fn submit_extra_rule(&mut self, cx: &mut Context<Self>) {
        // 与预览共用同一处推导，保证「看到什么就加什么」。
        let raw = self.extra_rule_payload.read(cx).content().trim().to_owned();
        let Some(draft) = draft_extra_rule(self, &raw) else {
            self.extra_rule_error = Some(t!("extra.unrecognized").into_owned());
            cx.notify();
            return;
        };
        let rule = ExtraRule {
            id: Uuid::new_v4().to_string(),
            ..draft
        };

        if rule.rule_type.is_empty() || rule.target.is_empty() {
            self.extra_rule_error = Some(t!("rules.extra_incomplete").into_owned());
            cx.notify();
            return;
        }
        let text = rule.to_rule_text();
        if let Err(error) = validate_rule_text(&text) {
            self.extra_rule_error =
                Some(t!("rules.extra_invalid", error = error.to_string()).into_owned());
            cx.notify();
            return;
        }
        // 重复条目没有意义，还会让人误以为改动没生效。
        if self
            .config
            .extra_rules
            .iter()
            .any(|existing| existing.to_rule_text() == text)
        {
            self.extra_rule_error = Some(t!("rules.extra_duplicate").into_owned());
            cx.notify();
            return;
        }

        let mut desired = self.config.extra_rules.clone();
        desired.push(rule);
        self.extra_rule_editor_open = false;
        self.extra_rule_type_picker = false;
        self.extra_rule_target_picker = false;
        self.extra_rule_payload
            .update(cx, |input, cx| input.set_content(String::new(), cx));
        self.commit_extra_rules(desired, cx);
    }

    /// 删除一条附加规则。
    pub(super) fn remove_extra_rule(&mut self, id: String, cx: &mut Context<Self>) {
        let desired: Vec<ExtraRule> = self
            .config
            .extra_rules
            .iter()
            .filter(|rule| rule.id != id)
            .cloned()
            .collect();
        if desired.len() == self.config.extra_rules.len() {
            return;
        }
        self.commit_extra_rules(desired, cx);
    }

    /// 启用或停用一条附加规则；停用只影响运行时合并，条目本身保留。
    pub(super) fn toggle_extra_rule(&mut self, id: String, cx: &mut Context<Self>) {
        let mut desired = self.config.extra_rules.clone();
        let Some(target) = desired.iter_mut().find(|rule| rule.id == id) else {
            return;
        };
        target.enabled = !target.enabled;
        self.commit_extra_rules(desired, cx);
    }

    /// 提交附加规则变更：app.json → 基线 → `-t` → runtime → 内核热重载。
    fn commit_extra_rules(&mut self, desired: Vec<ExtraRule>, cx: &mut Context<Self>) {
        let previous_rules = std::mem::replace(&mut self.config.extra_rules, desired);
        // app.json 是唯一事实来源；保存失败时不产生任何后续变更。
        if let Err(error) = self.config.save(&self.paths.config_file) {
            self.config.extra_rules = previous_rules;
            log_warn!("app", "保存全局附加规则失败：{error:#}");
            self.extra_rule_error = Some(concise_error(&format!("{error:#}"), 160));
            cx.notify();
            return;
        }

        let rollback_baseline = self.baseline.clone();
        let rollback_runtime = std::fs::read_to_string(&self.paths.runtime_mihomo_config_file).ok();
        let current_rules = self.config.extra_rules.clone();
        let reload = match self.write_extra_rules_configuration(&current_rules) {
            Ok(reload) => reload,
            Err(error) => {
                self.restore_extra_rules(previous_rules, rollback_baseline, rollback_runtime);
                log_error!("app", "应用全局附加规则失败：{error:#}");
                self.extra_rule_error = Some(extra_rule_rollback_error(&error));
                cx.notify();
                return;
            }
        };

        // 内核运行中热重载；正在启动的内核下一次重启即从磁盘生效。
        let needs_reload = reload.is_some();
        let total = self.config.extra_rules.len();
        let enabled = enabled_extra_rule_texts(&self.config.extra_rules).len();
        if let Some(runtime) = reload
            && self.mihomo_running()
            && let Some(controller) = self.controller()
        {
            cx.spawn(async move |this, cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move { controller.reload_config(&runtime) })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    match result {
                        Ok(()) => {
                            log_info!(
                                "profile",
                                "全局附加规则已热重载（共 {total} 条，启用 {enabled} 条）"
                            );
                            this.extra_rule_error = None;
                            this.extra_rule_notice = Some(t!("extra.applied").into_owned());
                        }
                        Err(error) => {
                            log_warn!("app", "运行中内核重载全局附加规则失败，回滚设置：{error:#}");
                            this.restore_extra_rules(
                                previous_rules,
                                rollback_baseline,
                                rollback_runtime,
                            );
                            this.extra_rule_error = Some(extra_rule_rollback_error(&error));
                            this.extra_rule_notice = None;
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
            return;
        }

        // 走到这里有两种情况：启用集合没变（runtime 无需重写），或内核未运行
        // （配置已落盘、下次启动生效）。两者都要回显结果，不能静默。
        log_info!(
            "profile",
            "全局附加规则已更新（共 {total} 条，启用 {enabled} 条）"
        );
        self.extra_rule_error = None;
        let notice = if needs_reload {
            t!("extra.saved_pending_kernel")
        } else {
            t!("extra.saved")
        };
        self.extra_rule_notice = Some(notice.into_owned());
        cx.notify();
    }

    /// 把已启用规则写入基线与 runtime；返回需要热重载的 runtime 文本。
    ///
    /// 启用集合没有变化时返回 `None`：例如新增一条停用规则，runtime 无需重写，
    /// 但 app.json 仍然已经落盘。
    fn write_extra_rules_configuration(
        &mut self,
        rows: &[ExtraRule],
    ) -> anyhow::Result<Option<String>> {
        let Some(previous_baseline) = self.baseline.clone() else {
            anyhow::bail!("本地基线不可用，无法写入附加规则");
        };
        let mut baseline = previous_baseline.clone();
        baseline.extra_rules = enabled_extra_rule_texts(rows);
        if baseline.extra_rules == previous_baseline.extra_rules {
            return Ok(None);
        }

        let runtime = profile::prepare_runtime(
            &self.paths,
            Some(&baseline),
            &self.config.mihomo_version,
            self.active_profile.as_deref(),
        )?;
        save_baseline(&self.paths, &baseline)?;
        if let Err(error) = write_runtime(&self.paths, &runtime) {
            // runtime 写入失败时回滚基线，保持磁盘一致。
            if let Err(restore) = save_baseline(&self.paths, &previous_baseline) {
                log_warn!("app", "回滚本地基线失败：{restore:#}");
            }
            return Err(error);
        }
        self.baseline = Some(baseline);
        Ok(Some(runtime))
    }

    /// 回滚附加规则带来的全部变更：内存、app.json、local.yaml 与 runtime.yaml。
    fn restore_extra_rules(
        &mut self,
        rules: Vec<ExtraRule>,
        baseline: Option<LocalBaseline>,
        runtime: Option<String>,
    ) {
        self.config.extra_rules = rules;
        if let Err(error) = self.config.save(&self.paths.config_file) {
            log_warn!("app", "回滚 app.json 失败：{error:#}");
        }
        if let Some(baseline) = baseline {
            if let Err(error) = save_baseline(&self.paths, &baseline) {
                log_warn!("app", "回滚本地基线失败：{error:#}");
            }
            self.baseline = Some(baseline);
        }
        if let Some(runtime) = runtime.as_deref()
            && let Err(error) = write_runtime(&self.paths, runtime)
        {
            log_warn!("app", "回滚运行时配置失败：{error:#}");
        }
    }
}

/// 编辑器当前会产出的规则。
///
/// 预览与提交共用这一处推导：所见即所得不是靠两份代码保持一致，而是只有一份。
/// 类型取自界面字段（由识别回填，用户可手动改），内容与目标取自识别结果。
pub(super) fn draft_extra_rule(app: &PureClash, raw: &str) -> Option<ExtraRule> {
    let (_recognized_type, payload, recognized_target) = recognize_rule_text(raw)?;
    Some(ExtraRule {
        id: String::new(),
        rule_type: app.extra_rule_type.trim().to_owned(),
        payload,
        target: if recognized_target.is_empty() {
            app.extra_rule_target.trim().to_owned()
        } else {
            recognized_target
        },
        enabled: true,
    })
}

/// 附加规则未能生效时的统一提示：明确告知已恢复原设置，避免界面与内核不一致。
fn extra_rule_rollback_error(error: &anyhow::Error) -> String {
    t!(
        "rules.extra_rollback",
        error = concise_error(&format!("{error:#}"), 160)
    )
    .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognize_rule_text_handles_paste_domain_and_url() {
        // 三段完整规则：拆分并把类型名规范成表内写法。
        assert_eq!(
            recognize_rule_text("domain-suffix, google.com, DIRECT"),
            Some((
                "DOMAIN-SUFFIX".to_owned(),
                "google.com".to_owned(),
                "DIRECT".to_owned()
            ))
        );
        // 从浏览器地址栏直接复制：只取主机名。
        assert_eq!(
            recognize_rule_text("https://ai.venlacy.com/console/personal"),
            Some((
                "DOMAIN-SUFFIX".to_owned(),
                "ai.venlacy.com".to_owned(),
                String::new()
            ))
        );
        // 带端口、查询串与片段。
        assert_eq!(
            recognize_rule_text("http://host.example.com:8443/a?b=1#c"),
            Some((
                "DOMAIN-SUFFIX".to_owned(),
                "host.example.com".to_owned(),
                String::new()
            ))
        );
        assert_eq!(
            recognize_rule_text("host.example.com:8443"),
            Some((
                "DOMAIN-SUFFIX".to_owned(),
                "host.example.com".to_owned(),
                String::new()
            ))
        );
        // 掩码不得被当成路径切掉。
        assert_eq!(
            recognize_rule_text("1.2.3.4/24"),
            Some(("IP-CIDR".to_owned(), "1.2.3.4/24".to_owned(), String::new()))
        );
        assert_eq!(recognize_rule_text("2001:db8::/32").unwrap().0, "IP-CIDR6");
        assert_eq!(recognize_rule_text("8080").unwrap().0, "DST-PORT");
        // 内容 + 目标；以及已知类型 + 目标（无内容）。
        assert_eq!(
            recognize_rule_text("ads.example.com,REJECT"),
            Some((
                "DOMAIN-SUFFIX".to_owned(),
                "ads.example.com".to_owned(),
                "REJECT".to_owned()
            ))
        );
        assert_eq!(
            recognize_rule_text("MATCH,DIRECT"),
            Some(("MATCH".to_owned(), String::new(), "DIRECT".to_owned()))
        );
        // 四段规则与空输入拒绝识别。
        assert_eq!(
            recognize_rule_text("IP-CIDR,1.2.3.4/24,DIRECT,no-resolve"),
            None
        );
        assert_eq!(recognize_rule_text("   "), None);
    }
}
