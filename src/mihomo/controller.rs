//! Mihomo external controller 的 REST 客户端。
//!
//! controller 由内核启动配置注入，只监听本机回环地址；客户端以随机 secret
//! 通过 `Authorization: Bearer` 认证。所有方法都是阻塞 HTTP 调用，调用方
//! 必须放在 GPUI 后台线程执行，不得阻塞主线程。

use std::{collections::BTreeMap, io::Read, time::Duration};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::mihomo::config::LocalBaseline;

/// 下载订阅时的体积上限；正常订阅远小于该值，超出视为异常响应。
pub(crate) const MAX_DOWNLOAD_BYTES: u64 = 10 * 1024 * 1024;

/// 延迟测试默认探测地址；与主流 Clash 面板一致，返回 204。
pub(crate) const DELAY_TEST_URL: &str = "https://www.gstatic.com/generate_204";

/// 本机 controller 客户端；实例可跨线程共享，方法均可重入。
#[derive(Clone)]
pub(crate) struct Controller {
    base_url: String,
    secret: String,
    agent: ureq::Agent,
    /// 延迟测试的内核侧超时可达数秒，HTTP 超时必须宽于探测时长。
    slow_agent: ureq::Agent,
    /// 热重载可能让内核重新读取 Geo 数据，超时对齐 Clash Verge Rev 给足 60 秒。
    reload_agent: ureq::Agent,
    /// 整组延迟测试专用：节点越多探测越久，且长请求期间对端可能关闭
    /// keep-alive 连接，因此既放宽超时，也不复用连接。
    bulk_agent: ureq::Agent,
}

impl Controller {
    /// 根据本地基线构造 controller 客户端。
    pub(crate) fn new(baseline: &LocalBaseline) -> Self {
        let agent = ureq::AgentBuilder::new()
            // 本机回环调用，短超时足够；失败由调用方转为界面错误。
            .timeout(Duration::from_secs(3))
            .build();
        let slow_agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(10))
            .build();
        let reload_agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(60))
            .build();
        let bulk_agent = ureq::AgentBuilder::new()
            // 实测组内单个节点最快也要近百毫秒，慢节点会吃满 5 秒内核预算，
            // 分组可达数十个节点，共用 10 秒会让客户端先于内核超时。
            .timeout(Duration::from_secs(120))
            // 关闭连接复用：长请求期间对端会关闭空闲连接，复用即报连接级失败。
            .max_idle_connections(0)
            .build();
        Self {
            base_url: format!("http://{}", baseline.controller_addr),
            secret: baseline.secret.clone(),
            agent,
            slow_agent,
            reload_agent,
            bulk_agent,
        }
    }

    /// 探测内核 controller 是否就绪，返回内核版本号。
    pub(crate) fn version(&self) -> Result<String> {
        #[derive(Deserialize)]
        struct VersionResponse {
            version: String,
        }

        let value: VersionResponse = self
            .agent
            .get(&format!("{}/version", self.base_url))
            .set("Authorization", &self.authorization())
            .call()
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("无法连接 Mihomo controller")?
            .into_json()
            .context("controller 版本响应格式无效")?;
        Ok(value.version)
    }

    /// 读取当前运行配置摘要；目前客户端只关心运行模式。
    pub(crate) fn configs(&self) -> Result<ConfigSnapshot> {
        #[derive(Deserialize)]
        struct ConfigsResponse {
            #[serde(default)]
            mode: String,
            #[serde(default)]
            tun: Option<TunState>,
        }

        #[derive(Deserialize)]
        struct TunState {
            #[serde(default)]
            enable: bool,
        }

        let value: ConfigsResponse = self
            .agent
            .get(&format!("{}/configs", self.base_url))
            .set("Authorization", &self.authorization())
            .call()
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("无法读取 Mihomo 运行配置")?
            .into_json()
            .context("controller 配置响应格式无效")?;
        Ok(ConfigSnapshot {
            mode: parse_mode(&value.mode),
            // 内核可能在启动时静默降级 TUN（缺权限或驱动），以此核对真实生效状态。
            tun_enabled: value.tun.is_some_and(|tun| tun.enable),
        })
    }

    /// 切换运行模式：`rule` / `global` / `direct`。
    pub(crate) fn patch_mode(&self, mode: &str) -> Result<()> {
        self.agent
            .patch(&format!("{}/configs", self.base_url))
            .set("Authorization", &self.authorization())
            .set("Content-Type", "application/json")
            .send_json(ureq::json!({ "mode": mode }))
            .map_err(|error| anyhow::anyhow!("{error}"))
            .with_context(|| format!("无法切换运行模式到 {mode}"))?;
        Ok(())
    }

    /// 运行时切换进程匹配模式（`always` / `strict` / `off`）；内核热生效，
    /// 不重启进程、不断开存量连接。
    pub(crate) fn patch_find_process_mode(&self, mode: &str) -> Result<()> {
        self.agent
            .patch(&format!("{}/configs", self.base_url))
            .set("Authorization", &self.authorization())
            .set("Content-Type", "application/json")
            .send_json(ureq::json!({ "find-process-mode": mode }))
            .map_err(|error| anyhow::anyhow!("{error}"))
            .with_context(|| format!("无法切换进程匹配模式到 {mode}"))?;
        Ok(())
    }

    /// 触发内核进程内热重载：`PUT /configs?force=true` 携带完整 YAML 内容。
    /// 内核进程保持存活、既有连接不主动断开，新配置对后续连接生效。
    /// 用 `payload` 而非 `path`：内核对 path 做了 home 目录 / SAFE_PATHS
    /// 安全校验，而 runtime.yaml 位于 `config/mihomo`（home 外）必被拒绝；
    /// payload 直接在请求体内传内容，不受路径限制（本机回环 + secret 认证）。
    /// 失败由调用方回退整进程重启，与 Clash Verge Rev 的应用策略一致。
    pub(crate) fn reload_config(&self, runtime_yaml: &str) -> Result<()> {
        self.reload_agent
            .put(&format!("{}/configs", self.base_url))
            .query("force", "true")
            .set("Authorization", &self.authorization())
            .set("Content-Type", "application/json")
            .send_json(ureq::json!({ "payload": runtime_yaml }))
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("无法热重载内核配置")?;
        Ok(())
    }

    /// 读取全部代理与策略组；`mode` 用于决定是否包含 GLOBAL 组。
    pub(crate) fn proxies(&self, mode: Mode) -> Result<ProxiesSnapshot> {
        let value: ProxiesResponse = self
            .agent
            .get(&format!("{}/proxies", self.base_url))
            .set("Authorization", &self.authorization())
            .call()
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("无法读取代理列表")?
            .into_json()
            .context("controller 代理响应格式无效")?;

        Ok(ProxiesSnapshot {
            groups: group_snapshots(&value.proxies, mode),
        })
    }

    /// 恢复客户端记录的选择；`selections` 为完整分组名到节点名的映射。
    /// 节点不存在时选择该组第一项；空组、自动组、未记录的组不修改。
    /// 返回成功表示全部需要修改的组已提交；HTTP 失败立即返回错误供重试。
    pub(crate) fn restore_proxy_selections(
        &self,
        selections: &BTreeMap<String, String>,
    ) -> Result<()> {
        if selections.is_empty() {
            return Ok(());
        }
        // 即使处于规则模式，也恢复隐藏的 GLOBAL 组，保证切到全局后沿用选择。
        for group in self.proxies(Mode::Global)?.groups {
            if let Some(saved) = selections.get(&group.name)
                && let Some(target) = group.restored_node(saved)
                && group.now != target
            {
                self.select_proxy(&group.name, target)?;
            }
        }
        Ok(())
    }

    /// 在策略组内选择当前节点；只对 Selector 组有效。
    pub(crate) fn select_proxy(&self, group: &str, node: &str) -> Result<()> {
        self.agent
            .put(&format!(
                "{}/proxies/{}",
                self.base_url,
                uri_path_segment(group)
            ))
            .set("Authorization", &self.authorization())
            .set("Content-Type", "application/json")
            .send_json(ureq::json!({ "name": node }))
            .map_err(|error| anyhow::anyhow!("{error}"))
            .with_context(|| format!("无法在策略组 {group} 中选择节点"))?;
        Ok(())
    }

    /// 读取连接快照：活跃连接列表与会话累计流量，用于连接页与实时网速。
    pub(crate) fn connections(&self) -> Result<ConnectionsSnapshot> {
        let value: ConnectionsResponse = self
            .agent
            .get(&format!("{}/connections", self.base_url))
            .set("Authorization", &self.authorization())
            .call()
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("无法读取连接数据")?
            .into_json()
            .context("controller 连接响应格式无效")?;
        Ok(value.into())
    }

    /// 读取内核规则快照。数组顺序即内核的匹配顺序，规则页据此展示优先级。
    pub(crate) fn rules(&self) -> Result<RulesSnapshot> {
        let value: RulesResponse = self
            .agent
            .get(&format!("{}/rules", self.base_url))
            .set("Authorization", &self.authorization())
            .call()
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("无法读取规则数据")?
            .into_json()
            .context("controller 规则响应格式无效")?;
        Ok(RulesSnapshot { rules: value.rules })
    }

    /// 关闭单个连接。
    pub(crate) fn close_connection(&self, id: &str) -> Result<()> {
        self.agent
            .delete(&format!(
                "{}/connections/{}",
                self.base_url,
                uri_path_segment(id)
            ))
            .set("Authorization", &self.authorization())
            .call()
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("无法关闭连接")?;
        Ok(())
    }

    /// 关闭全部连接。
    pub(crate) fn close_all_connections(&self) -> Result<()> {
        self.agent
            .delete(&format!("{}/connections", self.base_url))
            .set("Authorization", &self.authorization())
            .call()
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("无法关闭全部连接")?;
        Ok(())
    }

    /// 延迟测试专用请求：内核重启窗口内 socket 会短暂不可达，重试一次即可跨越。
    ///
    /// `CoreState::Running` 只表示 controller 曾经就绪，不保证此刻仍在 accept：
    /// 切换配置、热重载与 TUN 切换都会短暂重建监听。连接被拒时 ureq 的错误文本
    /// 就是请求 URL，对用户没有意义，因此这里重试，并把最终失败换成可读文案。
    fn delay_get(&self, agent: &ureq::Agent, path: &str) -> Result<ureq::Response> {
        let url = format!("{}{path}", self.base_url);
        let attempt = || {
            agent
                .get(&url)
                .set("Authorization", &self.authorization())
                .call()
        };
        match attempt() {
            Ok(response) => Ok(response),
            Err(_) => {
                std::thread::sleep(Duration::from_millis(DELAY_RETRY_DELAY_MS));
                attempt().map_err(|_| anyhow::anyhow!(DELAY_TRANSPORT_ERROR))
            }
        }
    }

    /// 对单个代理执行延迟测试；失败（超时/拒绝）返回错误描述。
    pub(crate) fn proxy_delay(&self, name: &str) -> Result<u64> {
        let path = format!(
            "/proxies/{}/delay?url={}&timeout={DELAY_TIMEOUT_MS}",
            uri_path_segment(name),
            uri_path_segment(DELAY_TEST_URL)
        );
        let value: DelayResponse = self
            .delay_get(&self.slow_agent, &path)
            .with_context(|| format!("节点 {name} 延迟测试失败"))?
            .into_json()
            .context("延迟测试响应格式无效")?;
        let message = value
            .message
            .unwrap_or_else(|| "内核未返回延迟数据".to_owned());
        value.delay.with_context(|| message)
    }

    /// 对策略组内全部节点执行延迟测试；返回节点名到延迟毫秒的映射。
    pub(crate) fn group_delay(&self, name: &str) -> Result<BTreeMap<String, u64>> {
        let path = format!(
            "/group/{}/delay?url={}&timeout={DELAY_TIMEOUT_MS}",
            uri_path_segment(name),
            uri_path_segment(DELAY_TEST_URL)
        );
        let value: BTreeMap<String, u64> = self
            .delay_get(&self.bulk_agent, &path)
            .with_context(|| format!("策略组 {name} 延迟测试失败"))?
            .into_json()
            .context("策略组延迟测试响应格式无效")?;
        Ok(value)
    }

    /// 下载订阅并同时取回流量快照；机场不返回该响应头时快照为 `None`。
    ///
    /// `subscription-userinfo` 是机场通用约定，不是所有机场都提供，因此缺失
    /// 属于正常情况：调用方保留原有快照而不是清空，避免一次无头响应把已有
    /// 用量抹掉。
    pub(crate) fn download_subscription_with_usage(
        url: &str,
    ) -> Result<(String, Option<crate::config::SubscriptionUsage>)> {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build();
        let response = agent
            .get(url)
            .set("User-Agent", "clash.meta/pure-clash")
            .call()
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("订阅下载失败")?;

        let usage = response
            .header("subscription-userinfo")
            .and_then(parse_subscription_userinfo);

        let length = response
            .header("Content-Length")
            .and_then(|length| length.parse::<u64>().ok());
        if let Some(length) = length
            && length > MAX_DOWNLOAD_BYTES
        {
            bail!("订阅内容超过 10MB 体积上限");
        }

        let mut reader = response.into_reader().take(MAX_DOWNLOAD_BYTES + 1);
        let mut body = Vec::new();
        reader.read_to_end(&mut body).context("订阅下载中断")?;
        if body.len() as u64 > MAX_DOWNLOAD_BYTES {
            bail!("订阅内容超过 10MB 体积上限");
        }

        let text = String::from_utf8(body)
            .map_err(|_| anyhow::anyhow!("订阅内容不是有效的 UTF-8 文本"))?;
        Ok((text, usage))
    }

    /// 只读取订阅的用量响应头，不消费正文。
    ///
    /// 与 `download_subscription_with_usage` 的区别：这里不做体积校验也不读
    /// 正文，因为调用方只要用量（配置页展示用），不需要内容。
    pub(crate) fn fetch_subscription_usage(
        url: &str,
    ) -> Result<Option<crate::config::SubscriptionUsage>> {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build();
        let response = agent
            .get(url)
            .set("User-Agent", "clash.meta/pure-clash")
            .call()
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("订阅用量查询失败")?;
        Ok(response
            .header("subscription-userinfo")
            .and_then(parse_subscription_userinfo))
    }

    fn authorization(&self) -> String {
        format!("Bearer {}", self.secret)
    }
}

/// 解析响应头 `subscription-userinfo`。
///
/// 约定格式为 `key=value; key=value`（例如
/// `upload=1; download=2; total=3; expire=1700000000`）。缺失或非法字段按 0
/// 处理；整段解析不出任何已知键时返回 `None`，让调用方区分“机场没给”与
/// “给了但全是 0”。
fn parse_subscription_userinfo(raw: &str) -> Option<crate::config::SubscriptionUsage> {
    let mut upload = 0u64;
    let mut download = 0u64;
    let mut total = 0u64;
    let mut expire_at = 0u64;
    let mut recognized = false;
    for field in raw.split(';') {
        let Some((key, value)) = field.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        // 机场可能给出带小数的字节数，取整数部分即可满足展示精度。
        let parsed = value
            .split('.')
            .next()
            .and_then(|head| head.parse::<u64>().ok());
        let Some(parsed) = parsed else {
            continue;
        };
        match key.as_str() {
            "upload" => {
                upload = parsed;
                recognized = true;
            }
            "download" => {
                download = parsed;
                recognized = true;
            }
            "total" => {
                total = parsed;
                recognized = true;
            }
            "expire" => {
                expire_at = parsed;
                recognized = true;
            }
            _ => {}
        }
    }
    recognized.then(|| crate::config::SubscriptionUsage {
        upload,
        download,
        total,
        expire_at,
        fetched_at: crate::profile::now_secs(),
    })
}

/// 运行模式；与 controller 的 `mode` 字段一一对应。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// 按配置规则分流，也是默认模式。
    Rule,
    /// 所有请求使用 GLOBAL 策略组。
    Global,
    /// 所有请求直接连接。
    Direct,
}

impl Mode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Rule => "rule",
            Self::Global => "global",
            Self::Direct => "direct",
        }
    }

    pub(crate) fn from_str(value: &str) -> Self {
        match value {
            "global" => Self::Global,
            "direct" => Self::Direct,
            _ => Self::Rule,
        }
    }
}

fn parse_mode(value: &str) -> Mode {
    Mode::from_str(value)
}

/// 运行配置摘要。
#[derive(Clone, Copy, Debug)]
pub(crate) struct ConfigSnapshot {
    pub(crate) mode: Mode,
    /// 内核侧 TUN 的真实生效状态。
    pub(crate) tun_enabled: bool,
}

/// 延迟测试的内核侧超时；与主流面板默认值一致。
pub(crate) const DELAY_TIMEOUT_MS: u64 = 5000;

/// 延迟测试的传输层失败文案。
///
/// ureq 的传输错误 Display 只给出请求 URL：把百分号编码的地址甩给用户等于
/// 没有提示，也解释不了失败原因；这里换成可读说明。
pub(crate) const DELAY_TRANSPORT_ERROR: &str = "测速请求未能送达内核：超时或被拒绝";

/// 连接被拒后重试前的等待；内核重建监听通常在数秒内完成。
pub(crate) const DELAY_RETRY_DELAY_MS: u64 = 400;

/// controller `/connections` 响应。
#[derive(Deserialize)]
struct ConnectionsResponse {
    #[serde(rename = "downloadTotal", default)]
    download_total: Option<u64>,
    #[serde(rename = "uploadTotal", default)]
    upload_total: Option<u64>,
    #[serde(default)]
    connections: Option<Vec<ConnectionItem>>,
}

/// 连接与流量快照。
#[derive(Clone, Debug, Default)]
pub(crate) struct ConnectionsSnapshot {
    /// 内核启动以来的累计下载字节数。
    pub(crate) download_total: u64,
    /// 内核启动以来的累计上传字节数。
    pub(crate) upload_total: u64,
    pub(crate) connections: Vec<ConnectionItem>,
}

impl From<ConnectionsResponse> for ConnectionsSnapshot {
    fn from(response: ConnectionsResponse) -> Self {
        Self {
            download_total: response.download_total.unwrap_or(0),
            upload_total: response.upload_total.unwrap_or(0),
            connections: response.connections.unwrap_or_default(),
        }
    }
}

/// 单条活跃连接。
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ConnectionItem {
    #[serde(default)]
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) upload: u64,
    #[serde(default)]
    pub(crate) download: u64,
    #[serde(default)]
    pub(crate) start: String,
    /// 代理链路数组；首个元素为实际出口节点。
    #[serde(default)]
    pub(crate) chains: Vec<String>,
    #[serde(default)]
    pub(crate) rule: String,
    /// 内核 JSON 字段是驼峰 `rulePayload`；漏掉显式映射会让规则内容静默丢失，
    /// 使「规则」列只剩类型、规则页的命中统计恒为空。
    #[serde(rename = "rulePayload", default)]
    pub(crate) rule_payload: String,
    #[serde(default)]
    pub(crate) metadata: ConnectionMetadata,
}

impl ConnectionItem {
    /// 界面展示的目标地址：优先嗅探到的域名，回退目标 IP:端口。
    pub(crate) fn target(&self) -> String {
        let metadata = &self.metadata;
        if !metadata.host.is_empty() {
            return format!("{}:{}", metadata.host, metadata.destination_port);
        }
        if !metadata.destination_ip.is_empty() {
            return format!("{}:{}", metadata.destination_ip, metadata.destination_port);
        }
        "—".to_owned()
    }

    /// 出口节点；无链路时回退 DIRECT。
    pub(crate) fn chain(&self) -> &str {
        self.chains.first().map(String::as_str).unwrap_or("DIRECT")
    }

    /// 命中规则与内容；规则为空（如直连探测）时显示占位。
    pub(crate) fn rule_label(&self) -> String {
        if self.rule.is_empty() {
            return "—".to_owned();
        }
        if self.rule_payload.is_empty() {
            self.rule.clone()
        } else {
            format!("{} · {}", self.rule, self.rule_payload)
        }
    }
}

/// 连接元数据；字段来自内核实测响应，未知字段直接忽略。
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct ConnectionMetadata {
    #[serde(default)]
    pub(crate) network: String,
    #[serde(default)]
    pub(crate) host: String,
    #[serde(rename = "destinationIP", default)]
    pub(crate) destination_ip: String,
    #[serde(rename = "destinationPort", default)]
    pub(crate) destination_port: String,
    #[serde(default)]
    pub(crate) process: String,
}

/// controller `/rules` 响应。
#[derive(Deserialize)]
struct RulesResponse {
    /// 内核始终返回数组；字段缺失按空列表处理。
    #[serde(default)]
    rules: Vec<RuleItem>,
}

/// 规则快照。
#[derive(Clone, Debug, Default)]
pub(crate) struct RulesSnapshot {
    /// 内核当前生效的规则；顺序即匹配顺序，越靠前优先级越高。
    pub(crate) rules: Vec<RuleItem>,
}

/// 单条规则。`type` 是 Rust 关键字，用 `rule_type` 承载并映射回响应字段。
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct RuleItem {
    #[serde(rename = "type", default)]
    pub(crate) rule_type: String,
    #[serde(default)]
    pub(crate) payload: String,
    /// 命中后的出口：代理组名、DIRECT、REJECT 或具体节点名。
    #[serde(default)]
    pub(crate) proxy: String,
    /// 规则在完整规则表中的位置（0 基）。禁用规则会被内核过滤，下标因此可能
    /// 出现空档；展示内核给出的下标比数组序号更贴近真实的匹配顺序。
    #[serde(default)]
    pub(crate) index: Option<u32>,
    /// 内核侧统计；`hitCount` 是自内核启动以来的累计命中次数。
    #[serde(default)]
    pub(crate) extra: RuleExtra,
}

/// 规则的内核侧统计信息；目前只取累计命中次数。
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct RuleExtra {
    #[serde(rename = "hitCount", default)]
    pub(crate) hit_count: u64,
}

impl RuleItem {
    /// 规则标识 `(类型, 内容)`；与连接快照的 `rule`/`rulePayload` 对齐，
    /// 规则页用它统计活跃连接的实时命中数。
    pub(crate) fn key(&self) -> (&str, &str) {
        (self.rule_type.as_str(), self.payload.as_str())
    }

    /// 界面展示的序号（1 基）。内核未给出下标时回退到数组位置。
    pub(crate) fn display_index(&self, fallback: usize) -> u32 {
        self.index.map_or(fallback as u32 + 1, |index| index + 1)
    }

    /// 界面展示的规则内容；无内容的规则（如 MATCH）只显示类型。
    pub(crate) fn rule_label(&self) -> String {
        if self.rule_type.is_empty() {
            return "—".to_owned();
        }
        if self.payload.is_empty() {
            self.rule_type.clone()
        } else {
            format!("{} · {}", self.rule_type, self.payload)
        }
    }
}

/// 延迟测试响应；失败时内核可能只返回 message。
#[derive(Deserialize)]
struct DelayResponse {
    #[serde(default)]
    delay: Option<u64>,
    #[serde(default)]
    message: Option<String>,
}

/// controller `/proxies` 响应。
#[derive(Deserialize)]
struct ProxiesResponse {
    #[serde(default)]
    proxies: std::collections::HashMap<String, ProxyItem>,
}

/// controller `/proxies` 响应中的单个代理或策略组条目。
#[derive(Deserialize)]
struct ProxyItem {
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    now: Option<String>,
    #[serde(default)]
    all: Vec<String>,
    /// 延迟测试历史；取最近一次有效值作为界面展示的缓存延迟。
    #[serde(default)]
    history: Vec<DelayHistory>,
}

/// 延迟测试历史条目。
#[derive(Deserialize)]
struct DelayHistory {
    #[serde(default)]
    delay: u64,
}

/// 将 controller 的代理映射整理为分组快照；组名排序、GLOBAL 置顶。
fn group_snapshots(
    proxies: &std::collections::HashMap<String, ProxyItem>,
    mode: Mode,
) -> Vec<GroupSnapshot> {
    let group_names: std::collections::HashSet<&str> = proxies
        .iter()
        .filter(|(_, item)| is_group_type(&item.r#type))
        .map(|(name, _)| name.as_str())
        .collect();
    let mut groups: Vec<GroupSnapshot> = proxies
        .iter()
        .filter(|(_, item)| is_group_type(&item.r#type))
        // GLOBAL 只在全局模式下参与分流，规则/直连模式不展示。
        .filter(|(name, _)| mode == Mode::Global || name.as_str() != "GLOBAL")
        .map(|(name, item)| {
            let nodes = item
                .all
                .iter()
                // GLOBAL 由内核自动生成，成员是全部代理与策略组的并集；
                // 策略组在页面已有独立分区，这里不再作为 GLOBAL 的节点展示。
                .filter(|node| name != "GLOBAL" || !group_names.contains(node.as_str()))
                .map(|node| {
                    proxies.get(node).map_or_else(
                        || NodeSnapshot {
                            name: node.clone(),
                            kind: String::new(),
                            delay: None,
                        },
                        |proxy| NodeSnapshot {
                            name: node.clone(),
                            kind: proxy.r#type.clone(),
                            delay: last_delay(&proxy.history),
                        },
                    )
                })
                .collect();
            GroupSnapshot {
                name: name.clone(),
                now: item.now.clone().unwrap_or_default(),
                // 只有 Selector 允许用户手动选择节点。
                selectable: item.r#type == "Selector",
                nodes,
            }
        })
        .collect();
    // 控制器返回的是无序映射，组名排序保证界面稳定。
    groups.sort_by(|left, right| left.name.cmp(&right.name));
    // GLOBAL 组置顶，便于全局模式下快速定位。
    if let Some(position) = groups.iter().position(|group| group.name == "GLOBAL") {
        let global = groups.remove(position);
        groups.insert(0, global);
    }
    groups
}

/// 代理组快照。
#[derive(Clone, Debug)]
pub(crate) struct GroupSnapshot {
    pub(crate) name: String,
    /// 当前选中的节点名；自动组同样提供该字段。
    pub(crate) now: String,
    /// 是否允许用户手动选择（Selector）。
    pub(crate) selectable: bool,
    pub(crate) nodes: Vec<NodeSnapshot>,
}

impl GroupSnapshot {
    /// 按完整名称恢复手动选择；不按下标或延迟匹配，避免排序变化后选错节点。
    fn restored_node(&self, saved: &str) -> Option<&str> {
        if !self.selectable {
            return None;
        }
        self.nodes
            .iter()
            .find(|node| node.name == saved)
            .or_else(|| self.nodes.first())
            .map(|node| node.name.as_str())
    }
}

/// 代理节点快照。
#[derive(Clone, Debug)]
pub(crate) struct NodeSnapshot {
    pub(crate) name: String,
    /// 节点协议类型，如 `vless`、`ss`；未知时为空。
    pub(crate) kind: String,
    /// 最近一次延迟测试结果（毫秒）；从未测试或测试失败时为空。
    pub(crate) delay: Option<u64>,
}

/// 取延迟历史中最近一次有效延迟；内核会把失败记为 0，需要跳过。
fn last_delay(history: &[DelayHistory]) -> Option<u64> {
    history
        .iter()
        .rev()
        .find(|entry| entry.delay > 0)
        .map(|entry| entry.delay)
}

/// 代理页数据快照。
#[derive(Clone, Debug, Default)]
pub(crate) struct ProxiesSnapshot {
    pub(crate) groups: Vec<GroupSnapshot>,
}

/// 判断代理类型是否为策略组；普通节点类型不在分组视图展示。
fn is_group_type(kind: &str) -> bool {
    matches!(
        kind,
        "Selector" | "URLTest" | "Fallback" | "LoadBalance" | "Relay"
    )
}

/// 组名作为 URL 路径段时做百分号编码，避免中文或特殊字符破坏路由。
fn uri_path_segment(segment: &str) -> String {
    let mut encoded = String::new();
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_roundtrip() {
        for mode in [Mode::Rule, Mode::Global, Mode::Direct] {
            assert_eq!(Mode::from_str(mode.as_str()), mode);
        }
        // 未知值回退到规则模式。
        assert_eq!(Mode::from_str("unknown"), Mode::Rule);
    }

    #[test]
    fn node_restoration_matches_names_and_only_falls_back_when_missing() {
        let mut group = GroupSnapshot {
            name: "代理选择".into(),
            now: "第一项".into(),
            selectable: true,
            nodes: ["第一项", "香港 / 01", "日本 #2"]
                .into_iter()
                .map(|name| NodeSnapshot {
                    name: name.into(),
                    kind: "Shadowsocks".into(),
                    delay: None,
                })
                .collect(),
        };
        // 即使节点没有测速结果或列表顺序改变，也恢复同名节点，不擅自切换。
        assert_eq!(group.restored_node("香港 / 01"), Some("香港 / 01"));
        group.nodes.reverse();
        assert_eq!(group.restored_node("香港 / 01"), Some("香港 / 01"));
        assert_eq!(group.restored_node("已删除"), Some("日本 #2"));
        group.selectable = false;
        assert_eq!(group.restored_node("香港 / 01"), None);
        group.selectable = true;
        group.nodes.clear();
        assert_eq!(group.restored_node("香港 / 01"), None);
    }

    #[test]
    #[ignore = "使用随包 Mihomo 验证重启及热重载后的节点恢复；仅监听临时回环 controller"]
    fn restores_saved_nodes_with_real_kernel_without_native_cache() {
        use crate::{
            config::AppConfig,
            mihomo::{MihomoProcess, config::ensure_baseline},
            platform::AppPaths,
        };
        use std::{fs, net::TcpListener, path::PathBuf};

        let root =
            std::env::temp_dir().join(format!("pure-clash-node-restore-{}", uuid::Uuid::new_v4()));
        let mut paths = AppPaths::portable(&root);
        paths.kernel_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("kernel");
        fs::create_dir_all(&paths.mihomo_config_dir).unwrap();
        fs::create_dir_all(&paths.mihomo_data_dir).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let mut baseline = ensure_baseline(&paths).unwrap();
        baseline.controller_addr = address.to_string();
        let controller = Controller::new(&baseline);
        // 禁用内核缓存、代理端口、DNS 与 TUN，单独验证客户端恢复且不影响用户网络。
        let runtime = format!(
            "mixed-port: 0\nexternal-controller: {address}\nsecret: {}\nmode: rule\nprofile:\n  store-selected: false\nproxy-groups:\n- name: 手动组\n  type: select\n  proxies: [DIRECT, REJECT]\nrules:\n- MATCH,DIRECT\n",
            baseline.secret
        );
        fs::write(&paths.runtime_mihomo_config_file, &runtime).unwrap();
        let start = || {
            let process = MihomoProcess::start(
                &paths,
                env!("PURE_CLASH_DEFAULT_MIHOMO_VERSION"),
                &paths.runtime_mihomo_config_file,
                false,
                true,
            )
            .unwrap();
            for _ in 0..50 {
                if controller.version().is_ok() {
                    return process;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            panic!("测试 controller 未就绪");
        };
        let selected = |name: &str| {
            controller
                .proxies(Mode::Global)
                .unwrap()
                .groups
                .into_iter()
                .find(|group| group.name == name)
                .unwrap()
                .now
        };
        let mut process = start();
        let mut config = AppConfig::default();
        for name in ["手动组", "GLOBAL"] {
            controller.select_proxy(name, "REJECT").unwrap();
            config.selected_nodes.insert(name.into(), "REJECT".into());
        }
        config
            .selected_nodes
            .insert("已删除的分组".into(), "不存在的节点".into());
        config.save(&paths.config_file).unwrap();
        process.stop().unwrap();

        // 重启后从 app.json 回读选择；此时内核原始默认值是第一项 DIRECT。
        let restored: AppConfig =
            serde_json::from_str(&fs::read_to_string(&paths.config_file).unwrap()).unwrap();
        let mut process = start();
        assert_eq!(selected("手动组"), "DIRECT");
        controller
            .restore_proxy_selections(&restored.selected_nodes)
            .unwrap();
        assert_eq!(selected("手动组"), "REJECT");
        assert_eq!(selected("GLOBAL"), "REJECT");

        // 订阅更新删除选中节点时回退首项，但不覆盖记忆，节点重新出现后仍可恢复。
        controller
            .reload_config(&runtime.replace("[DIRECT, REJECT]", "[DIRECT]"))
            .unwrap();
        controller
            .restore_proxy_selections(&restored.selected_nodes)
            .unwrap();
        assert_eq!(selected("手动组"), "DIRECT");
        controller.reload_config(&runtime).unwrap();
        controller
            .restore_proxy_selections(&restored.selected_nodes)
            .unwrap();
        assert_eq!(selected("手动组"), "REJECT");
        process.stop().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn group_type_detection() {
        assert!(is_group_type("Selector"));
        assert!(is_group_type("URLTest"));
        assert!(!is_group_type("Vless"));
        assert!(!is_group_type("Shadowsocks"));
    }

    #[test]
    fn parses_subscription_userinfo_header() {
        let usage =
            parse_subscription_userinfo("upload=100; download=200; total=1000; expire=1700000000")
                .expect("完整响应头应解析成功");
        assert_eq!(usage.upload, 100);
        assert_eq!(usage.download, 200);
        assert_eq!(usage.total, 1000);
        assert_eq!(usage.expire_at, 1_700_000_000);
        assert_eq!(usage.used(), 300);
        assert_eq!(usage.ratio(), Some(0.3));
    }

    #[test]
    fn tolerates_missing_and_odd_userinfo_fields() {
        // 只给部分字段：缺失项按 0，仍算识别成功。
        let partial = parse_subscription_userinfo("download=2048").expect("部分字段应可解析");
        assert_eq!(partial.download, 2048);
        assert_eq!(partial.total, 0);
        // 不限量机场：total=0 时没有可用进度条。
        assert_eq!(partial.ratio(), None);
        // 完全无关的内容不应被当成用量。
        assert!(parse_subscription_userinfo("hello").is_none());
        assert!(parse_subscription_userinfo("").is_none());
        // 带小数的字节数与大小写混排也要能吃下。
        let odd = parse_subscription_userinfo("UPLOAD=1.5; Download=2.9; TOTAL=10").unwrap();
        assert_eq!(odd.upload, 1);
        assert_eq!(odd.download, 2);
    }

    #[test]
    fn usage_days_left_handles_expiry() {
        let mut usage = crate::config::SubscriptionUsage {
            upload: 0,
            download: 0,
            total: 100,
            expire_at: 1_700_000_000,
            fetched_at: 0,
        };
        assert_eq!(usage.days_left(1_700_000_000 - 86_400 * 3), Some(3));
        // 已过期不返回负数。
        assert_eq!(usage.days_left(1_700_000_000 + 10), Some(0));
        // 无到期时间时未知。
        usage.expire_at = 0;
        assert_eq!(usage.days_left(1_700_000_000), None);
    }

    #[test]
    fn uri_segment_encodes_special_characters() {
        assert_eq!(uri_path_segment("PROXY"), "PROXY");
        assert_eq!(
            uri_path_segment("美国 主力"),
            "%E7%BE%8E%E5%9B%BD%20%E4%B8%BB%E5%8A%9B"
        );
        assert_eq!(uri_path_segment("a/b?c"), "a%2Fb%3Fc");
    }

    #[test]
    fn global_group_hides_nested_policy_groups() {
        // 模拟内核响应：GLOBAL 成员是全部代理与策略组的并集。
        let mut proxies = std::collections::HashMap::new();
        proxies.insert(
            "GLOBAL".to_string(),
            ProxyItem {
                r#type: "Selector".into(),
                now: Some("PROXY".into()),
                all: vec![
                    "DIRECT".into(),
                    "REJECT".into(),
                    "美国主力".into(),
                    "PROXY".into(),
                ],
                history: vec![],
            },
        );
        proxies.insert(
            "PROXY".to_string(),
            ProxyItem {
                r#type: "Selector".into(),
                now: Some("美国主力".into()),
                all: vec!["美国主力".into(), "美国备用".into()],
                history: vec![],
            },
        );
        proxies.insert(
            "DIRECT".to_string(),
            ProxyItem {
                r#type: "Direct".into(),
                now: None,
                all: vec![],
                history: vec![],
            },
        );
        proxies.insert(
            "REJECT".to_string(),
            ProxyItem {
                r#type: "Reject".into(),
                now: None,
                all: vec![],
                history: vec![],
            },
        );
        proxies.insert(
            "美国主力".to_string(),
            ProxyItem {
                r#type: "Vless".into(),
                now: None,
                all: vec![],
                history: vec![],
            },
        );
        proxies.insert(
            "美国备用".to_string(),
            ProxyItem {
                r#type: "Vless".into(),
                now: None,
                all: vec![],
                history: vec![],
            },
        );

        let groups = group_snapshots(&proxies, Mode::Global);
        // GLOBAL 置顶，内嵌的 PROXY 组不作为节点，内置 DIRECT/REJECT 保留。
        assert_eq!(groups[0].name, "GLOBAL");
        let global_nodes: Vec<&str> = groups[0]
            .nodes
            .iter()
            .map(|node| node.name.as_str())
            .collect();
        assert_eq!(global_nodes, vec!["DIRECT", "REJECT", "美国主力"]);

        // 订阅策略组不受影响，普通节点照常展示。
        let proxy_group = groups.iter().find(|group| group.name == "PROXY").unwrap();
        let proxy_nodes: Vec<&str> = proxy_group
            .nodes
            .iter()
            .map(|node| node.name.as_str())
            .collect();
        assert_eq!(proxy_nodes, vec!["美国主力", "美国备用"]);
        assert!(proxy_group.selectable);
    }

    #[test]
    fn global_group_only_visible_in_global_mode() {
        let mut proxies = std::collections::HashMap::new();
        proxies.insert(
            "GLOBAL".to_string(),
            ProxyItem {
                r#type: "Selector".into(),
                now: Some("DIRECT".into()),
                all: vec!["DIRECT".into()],
                history: vec![],
            },
        );
        proxies.insert(
            "DIRECT".to_string(),
            ProxyItem {
                r#type: "Direct".into(),
                now: None,
                all: vec![],
                history: vec![],
            },
        );

        assert_eq!(group_snapshots(&proxies, Mode::Global).len(), 1);
        // 规则/直连模式下不展示 GLOBAL 组。
        assert!(group_snapshots(&proxies, Mode::Rule).is_empty());
        assert!(group_snapshots(&proxies, Mode::Direct).is_empty());
    }

    #[test]
    fn last_delay_skips_failed_entries() {
        // 内核把失败延迟记为 0，界面应取最近一次成功值。
        let history = vec![
            DelayHistory { delay: 120 },
            DelayHistory { delay: 0 },
            DelayHistory { delay: 0 },
        ];
        assert_eq!(last_delay(&history), Some(120));
        assert_eq!(last_delay(&[DelayHistory { delay: 0 }]), None);
        assert_eq!(last_delay(&[]), None);
    }

    #[test]
    fn connections_snapshot_parses_probe_response() {
        // 结构来自 1.19.30 内核实机响应（含限速下载建立的活跃连接）。
        let body = r#"{
            "downloadTotal": 2015106,
            "uploadTotal": 3729,
            "memory": 0,
            "connections": [{
                "id": "980c3fab-ed27-434e-a2f2-388cbc6840d4",
                "metadata": {
                    "network": "tcp",
                    "type": "HTTPS",
                    "sourceIP": "127.0.0.1",
                    "destinationIP": "",
                    "sourcePort": "39956",
                    "destinationPort": "443",
                    "host": "speed.cloudflare.com",
                    "dnsMode": "normal",
                    "process": "",
                    "processPath": "",
                    "remoteDestination": "172.66.0.218"
                },
                "upload": 1790,
                "download": 2009967,
                "start": "2026-08-30T15:35:09.826432755+08:00",
                "chains": ["DIRECT"],
                "providerChains": [""],
                "rule": "",
                "rulePayload": ""
            }]
        }"#;
        let snapshot: ConnectionsSnapshot = serde_json::from_str::<ConnectionsResponse>(body)
            .unwrap()
            .into();
        assert_eq!(snapshot.download_total, 2015106);
        assert_eq!(snapshot.upload_total, 3729);
        assert_eq!(snapshot.connections.len(), 1);
        let connection = &snapshot.connections[0];
        assert_eq!(connection.target(), "speed.cloudflare.com:443");
        assert_eq!(connection.chain(), "DIRECT");
        assert_eq!(connection.rule_label(), "—");
    }

    #[test]
    fn connections_snapshot_tolerates_null_and_missing_fields() {
        // 空闲内核返回 connections: null；null 解析为 None，快照转换折叠为空数组。
        let response: ConnectionsResponse =
            serde_json::from_str(r#"{"downloadTotal": 5, "connections": null}"#).unwrap();
        let snapshot: ConnectionsSnapshot = response.into();
        assert!(snapshot.connections.is_empty());
        assert_eq!(snapshot.download_total, 5);

        let minimal: ConnectionsResponse = serde_json::from_str("{}").unwrap();
        assert_eq!(minimal.download_total, None);

        let snapshot: ConnectionsSnapshot = minimal.into();
        assert_eq!(snapshot.download_total, 0);
    }

    #[test]
    fn connection_target_falls_back_to_destination_ip() {
        let metadata = ConnectionMetadata {
            network: "tcp".into(),
            host: String::new(),
            destination_ip: "1.2.3.4".into(),
            destination_port: "80".into(),
            process: "curl".into(),
        };
        let connection = ConnectionItem {
            id: "id".into(),
            upload: 1,
            download: 2,
            start: String::new(),
            chains: vec!["美国 01".into()],
            rule: "Match".into(),
            rule_payload: String::new(),
            metadata,
        };
        assert_eq!(connection.target(), "1.2.3.4:80");
        assert_eq!(connection.chain(), "美国 01");
        assert_eq!(connection.rule_label(), "Match");
        // 规则带内容时合并展示。
        let mut with_payload = connection.clone();
        with_payload.rule_payload = "google".into();
        assert_eq!(with_payload.rule_label(), "Match · google");
    }

    #[test]
    fn connection_rule_payload_parses_from_kernel_camel_case() {
        // 回归测试：内核字段是驼峰 `rulePayload`，字段名缺显式映射就会静默丢内容。
        // 上面的测试用结构体字面量直接赋值，绕过了反序列化；另一个 serde 测试的
        // fixture 里规则内容恰好为空串，因此两者都无法发现该问题 —— 这里必须走
        // 真实 JSON 解析，并且使用非空内容。
        let connection: ConnectionItem =
            serde_json::from_str(r#"{"rule": "DomainSuffix", "rulePayload": "example.com"}"#)
                .unwrap();
        assert_eq!(connection.rule, "DomainSuffix");
        assert_eq!(connection.rule_payload, "example.com");
        assert_eq!(connection.rule_label(), "DomainSuffix · example.com");
    }

    #[test]
    fn rules_snapshot_parses_kernel_response() {
        // 结构对齐 1.19.30 内核 `/rules` 响应；数组顺序即匹配顺序。
        let body = r#"{
            "rules": [
                {"type": "DomainSuffix", "payload": "example.com", "proxy": "PROXY"},
                {"type": "GeoIP", "payload": "CN", "proxy": "DIRECT"},
                {"type": "Match", "payload": "", "proxy": "PROXY"}
            ]
        }"#;
        let response: RulesResponse = serde_json::from_str(body).unwrap();
        let snapshot = RulesSnapshot {
            rules: response.rules,
        };
        assert_eq!(snapshot.rules.len(), 3);
        assert_eq!(snapshot.rules[0].key(), ("DomainSuffix", "example.com"));
        assert_eq!(snapshot.rules[0].rule_label(), "DomainSuffix · example.com");
        assert_eq!(snapshot.rules[0].proxy, "PROXY");
        // 无内容的规则（如 MATCH）只显示类型，不出现悬空分隔符。
        assert_eq!(snapshot.rules[2].rule_label(), "Match");
    }

    #[test]
    fn rules_snapshot_tolerates_missing_rules_field() {
        let response: RulesResponse = serde_json::from_str("{}").unwrap();
        assert!(
            RulesSnapshot {
                rules: response.rules
            }
            .rules
            .is_empty()
        );
    }

    #[test]
    fn rule_key_aligns_with_connection_rule_fields() {
        // 命中统计依赖两侧字段一一对齐：规则侧 `type`/`payload`，连接侧
        // `rule`/`rulePayload`。任一侧改名或丢映射都会让命中列静默归零。
        let rule: RuleItem =
            serde_json::from_str(r#"{"type": "DomainSuffix", "payload": "example.com"}"#).unwrap();
        let connection: ConnectionItem =
            serde_json::from_str(r#"{"rule": "DomainSuffix", "rulePayload": "example.com"}"#)
                .unwrap();
        assert_eq!(
            rule.key(),
            (connection.rule.as_str(), connection.rule_payload.as_str())
        );
        assert_eq!(rule.rule_label(), connection.rule_label());
    }

    #[test]
    fn rules_snapshot_parses_real_kernel_payload_with_stats() {
        // 原文来自 1.19.30 内核实机 `GET /rules`（默认配置只有一条 MATCH 规则）。
        // 该响应还带 size / hitAt / missCount 等本客户端不使用的字段，解析必须忽略。
        let body = r#"{"rules":[{"index":0,"type":"Match","payload":"","proxy":"PROXY",
            "size":-1,"extra":{"disabled":false,"hitCount":0,
            "hitAt":"1970-01-01T08:00:00+08:00","missCount":0,
            "missAt":"1970-01-01T08:00:00+08:00"}}]}"#;
        let response: RulesResponse = serde_json::from_str(body).unwrap();
        let snapshot = RulesSnapshot {
            rules: response.rules,
        };
        let rule = &snapshot.rules[0];
        assert_eq!(rule.rule_type, "Match");
        assert_eq!(rule.proxy, "PROXY");
        assert_eq!(rule.index, Some(0));
        assert_eq!(rule.display_index(7), 1);
        assert_eq!(rule.extra.hit_count, 0);
        assert_eq!(rule.rule_label(), "Match");
    }

    #[test]
    fn rule_display_index_falls_back_to_array_position() {
        // 内核未给出下标（旧版本或字段缺失）时回退到数组位置，仍然从 1 开始。
        let rule: RuleItem = serde_json::from_str(r#"{"type": "GeoIP", "payload": "CN"}"#).unwrap();
        assert_eq!(rule.index, None);
        assert_eq!(rule.display_index(0), 1);
        assert_eq!(rule.display_index(4), 5);
        assert_eq!(rule.extra.hit_count, 0);
    }
}
