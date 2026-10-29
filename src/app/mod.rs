use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, IntoElement, PathPromptOptions, Render,
    SharedString, Styled, Window, div, prelude::*, px, rgb,
};
use rust_i18n::t;

use crate::{
    assets::{
        ICON_EYE, ICON_EYE_OFF, ICON_FILE_CODE, ICON_GIT_BRANCH, ICON_INFO, ICON_LAYOUT,
        ICON_MESSAGE, ICON_PLUS, ICON_SETTINGS, ICON_SHIELD_CHECK,
    },
    config::{AppConfig, Language, LoadedConfig, ProfileMeta},
    kernel::is_available,
    mihomo::{
        MihomoProcess,
        config::NodeEndpoint,
        config::ensure_baseline,
        config::merge_runtime,
        config::read_node_endpoints,
        config::read_proxy_group_order,
        config::save_baseline,
        config::validate_kernel_config,
        config::write_runtime,
        controller::{
            ConnectionItem, ConnectionsSnapshot, Controller, GroupSnapshot, Mode, NodeSnapshot,
            RuleItem,
        },
        geodata::{GeodataInfo, UpdateOutcome},
    },
    platform::{
        AppPaths, AutoStartStatus, SystemProxySnapshot, autostart_status, capture_system_proxy,
        restore_system_proxy, set_autostart, set_system_proxy,
    },
    profile,
    startup::StartupMode,
    theme::{FontWeightExt, Palette},
    ui::TextInput,
};

impl PureClash {
    /// 应用当前版本；与 Cargo 包版本保持一致，供更新检查比较。
    pub(crate) const CURRENT_VERSION: &'static str = env!("CARGO_PKG_VERSION");
}

/// 超过该节点数的代理分组默认折叠，避免大订阅进入代理页时一次性布局全部节点。
const PROXY_AUTO_COLLAPSE_NODES: usize = 30;

/// 未手动操作时允许自动展开的节点总量上界；超出后其余分组默认折叠。
const PROXY_AUTO_EXPAND_NODE_BUDGET: usize = 120;

/// 代理节点卡片一行展示的列数。
const PROXY_NODE_COLUMNS: u16 = 3;

/// 展开分组单页渲染的节点数上限；大分组通过“显示更多”分页浏览，
/// 让单次布局量有硬上界，同时滚动只保留页面一层。
const PROXY_NODE_PAGE_SIZE: usize = 30;

mod about;
mod connections;
mod extra_rules;
mod frame;
mod header;
mod net_check;
mod overview;
mod profiles;
mod proxies;
mod rules;
mod rules_ops;
mod settings;
mod shell;
mod sidebar;

// 阶段①拆分出的业务模块；impl 方法可见性提升为 pub(super) 以便 app 内互相调用。
mod core_runtime;
mod integration;
mod proxy_ops;
mod settings_ops;
mod subscriptions;
mod tun;
mod view_state;

pub(crate) use shell::AppShell;

use extra_rules::render_picker_overlay;
use profiles::{render_profile_editor_overlay, render_profile_menu};
#[cfg(target_os = "linux")]
use frame::linux_client_side_decorations;
use frame::render_titlebar;
use header::render_page;
use proxies::group_auto_expanded;
use sidebar::render_sidebar;

/// 应用侧边栏中的基础页面，覆盖日常代理管理的最小闭环。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Overview,
    Proxies,
    Rules,
    ExtraRules,
    Connections,
    Profiles,
    Settings,
    About,
}

impl Page {
    fn all() -> [Self; 7] {
        [
            Self::Overview,
            Self::Proxies,
            // 配置文件是高频操作（换订阅、看用量），紧随代理页之后。
            Self::Profiles,
            Self::Rules,
            Self::ExtraRules,
            Self::Connections,
            Self::Settings,
        ]
    }

    fn label(self) -> SharedString {
        match self {
            Self::Overview => tr("page.overview"),
            Self::Proxies => tr("page.proxies"),
            Self::Rules => tr("page.rules"),
            Self::ExtraRules => tr("page.extra_rules"),
            Self::Connections => tr("page.connections"),
            Self::Profiles => tr("page.profiles"),
            Self::Settings => tr("page.settings"),
            Self::About => tr("page.about"),
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Overview => ICON_LAYOUT,
            Self::Proxies => ICON_GIT_BRANCH,
            Self::Rules => ICON_SHIELD_CHECK,
            Self::ExtraRules => ICON_PLUS,
            Self::Connections => ICON_MESSAGE,
            Self::Profiles => ICON_FILE_CODE,
            Self::Settings => ICON_SETTINGS,
            Self::About => ICON_INFO,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProxyMode {
    Rule,
    Global,
    Direct,
}

impl ProxyMode {
    fn label(self) -> SharedString {
        match self {
            Self::Rule => tr("mode.rule"),
            Self::Global => tr("mode.global"),
            Self::Direct => tr("mode.direct"),
        }
    }

    fn detail(self) -> SharedString {
        match self {
            Self::Rule => tr("mode.rule_detail"),
            Self::Global => tr("mode.global_detail"),
            Self::Direct => tr("mode.direct_detail"),
        }
    }

    fn from_controller(mode: Mode) -> Self {
        match mode {
            Mode::Rule => Self::Rule,
            Mode::Global => Self::Global,
            Mode::Direct => Self::Direct,
        }
    }

    fn to_controller(self) -> Mode {
        match self {
            Self::Rule => Mode::Rule,
            Self::Global => Mode::Global,
            Self::Direct => Mode::Direct,
        }
    }
}

/// 配置文件页的展示方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProfilesView {
    /// 紧凑列表：单行信息，适合订阅多时快速扫视。
    List,
    /// 卡片网格：多列卡片，容纳用量进度条与到期信息。
    Grid,
}

impl ProfilesView {
    /// 在两种视图之间切换。
    fn toggled(self) -> Self {
        match self {
            Self::List => Self::Grid,
            Self::Grid => Self::List,
        }
    }

    fn is_grid(self) -> bool {
        matches!(self, Self::Grid)
    }
}

/// 内核运行状态；Starting 表示进程已拉起，正在等待 controller 就绪。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CoreState {
    Stopped,
    Starting,
    Running,
}

/// 开关的可见状态；Starting 表示授权/内核初始化尚未完成，不等同于已开启。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SwitchState {
    Off,
    Starting,
    On,
}

impl From<bool> for SwitchState {
    fn from(enabled: bool) -> Self {
        if enabled { Self::On } else { Self::Off }
    }
}

impl SwitchState {
    fn label(self) -> SharedString {
        tr(match self {
            Self::Off => "status.off",
            Self::Starting => "status.starting",
            Self::On => "status.on",
        })
    }
}

/// 停机后不得显示旧状态，内核启动阶段不得提前显示 TUN 已开启。
fn tun_visible_state(core_state: CoreState, state: SwitchState) -> SwitchState {
    match core_state {
        CoreState::Stopped => SwitchState::Off,
        CoreState::Starting if state != SwitchState::Off => SwitchState::Starting,
        _ => state,
    }
}

/// Windows 没有常驻提权服务，登录恢复 TUN 必须允许 UAC；Linux 登录阶段只允许
/// 访问已安装服务，避免桌面刚登录就弹出 polkit。
fn startup_allows_interactive_elevation(startup_mode: StartupMode) -> bool {
    cfg!(target_os = "windows") || !startup_mode.is_autostart()
}

/// 应用外壳用于刷新平台托盘的完整文案快照。
pub(super) struct TrayTexts {
    pub(super) tooltip: String,
    pub(super) open: String,
    pub(super) quit: String,
}

/// 连接页单次渲染的最大行数；超出部分提示条数，避免超大订阅拖慢布局。
const CONNECTIONS_RENDER_LIMIT: usize = 200;
/// 实时连接与流量的轮询间隔；与内核 dashboard 的默认推送节奏一致。
const CONNECTIONS_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
/// 订阅自动更新调度器的检查间隔；到期判断是墙钟比较，跳频只影响响应速度。
const PROFILE_UPDATE_TICK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

pub(crate) struct PureClash {
    page: Page,
    /// 启动时读取并在应用内持续维护的主配置对象。
    config: AppConfig,
    /// 当前平台的程序、配置、数据和内核目录集合。
    paths: AppPaths,
    /// 仅表示配置所选版本的内核文件存在，不代表 Mihomo 进程已经运行。
    kernel_available: bool,
    /// 当前由应用持有的真实 Mihomo 子进程，释放时会自动停止。
    mihomo_process: Option<MihomoProcess>,
    core_state: CoreState,
    /// 每次启动或停止都递增；后台返回的旧启动结果必须立即回收，不能覆盖新状态。
    core_start_generation: u64,
    /// UAC 尚未返回时合并配置重启请求，避免并发弹出多个授权窗口。
    core_restart_pending: bool,
    /// 最近一次内核启停错误，直接展示在运行状态卡片中。
    mihomo_error: Option<String>,
    system_proxy: bool,
    /// 本地基线中持久化的 TUN 期望值，决定下一次内核是否请求 TUN 权限。
    tun_configured: bool,
    /// TUN 授权与初始化全过程的状态；只有 controller 确认后才进入 On。
    tun_state: SwitchState,
    /// 登录自启状态始终来自平台配置，不复制到 `app.json`，避免与系统启动管理器分叉。
    autostart_enabled: bool,
    /// macOS 等尚未实现平台注册的平台显示禁用态，不能响应开关操作。
    autostart_available: bool,
    /// Linux 登录自启阶段为 false，避免弹 polkit；Windows 允许恢复已配置 TUN
    /// 所必需的 UAC。无交互启动失败时会关闭 TUN 配置并回退普通内核。
    interactive_elevation_allowed: bool,
    /// 运行模式；与 controller 同步，未运行时保持上次已知值。
    mode: ProxyMode,
    /// 模式切换串行执行，避免异步请求乱序覆盖已保存的选择。
    mode_switching: bool,
    /// 节点切换与配置更新互斥，防止旧请求结果写入新配置。
    node_switching: bool,
    /// 模式或节点操作失败时，在代理页提示用户。
    proxy_error: Option<String>,
    /// controller 返回的真实策略组快照；内核未运行时为空。
    groups: Vec<GroupSnapshot>,
    /// 运行配置请求失败边沿标记；与连接轮询独立，避免一条请求吞掉另一条日志。
    runtime_state_failing: bool,
    /// 连接轮询失败边沿标记：只在失败开始与恢复时各记一条日志。
    connections_failing: bool,
    /// controller 轮询到的活跃连接（按建立时间序）；内核停止时清空。
    connections: Vec<ConnectionItem>,
    /// 实时上传/下载速度与会话累计字节数；由相邻连接快照差分得出。
    traffic_down_speed: u64,
    traffic_up_speed: u64,
    download_total: u64,
    upload_total: u64,
    /// 内核规则快照；顺序即匹配顺序，进入规则页时按需拉取。
    rules: Vec<RuleItem>,
    /// 规则页数据拉取中。
    rules_loading: bool,
    /// 规则页拉取失败提示；展示在规则页空态中。
    rules_error: Option<String>,
    /// 规则页拉取失败边沿标记：只在失败开始与恢复时各记一条日志。
    rules_failing: bool,
    /// 规则列表已渲染的行数；点击“显示更多”按页递增。
    rules_shown: usize,
    /// 附加规则编辑器是否展开。
    extra_rule_editor_open: bool,
    /// 编辑器当前选中的规则类型。
    extra_rule_type: String,
    /// 编辑器中的规则内容输入框。
    extra_rule_payload: Entity<TextInput>,
    /// 编辑器当前选中的出口。
    extra_rule_target: String,
    /// 规则类型列表是否展开。
    extra_rule_type_picker: bool,
    /// 出口列表是否展开。
    extra_rule_target_picker: bool,
    /// 选择区当前渲染的选项数；点「显示更多」按页递增。
    extra_rule_options_shown: usize,
    /// 附加规则的校验与配置事务失败提示；展示在规则页附加规则卡片中。
    extra_rule_error: Option<String>,
    /// 配置文件页的展示方式；默认紧凑列表。
    profiles_view: ProfilesView,
    /// 是否正在补齐订阅用量；避免同批查询被重复发起。
    usage_refresh_in_flight: bool,
    /// 附加规则变更后的结果提示。热重载成功与「已保存待内核生效」都走这里，
    /// 避免变更结果静默——用户此前只能靠重启内核来判断规则是否生效。
    extra_rule_notice: Option<String>,
    /// 输入框的观察者：内容变化时让页面重绘，识别预览才能实时跟随。
    /// 订阅是 RAII 守卫，随实体一起存活，字段本身不参与读写。
    _extra_rule_input_observer: gpui::Subscription,
    /// 延迟测试进行中的节点名集合；测速按钮据此显示忙态。
    delay_testing: std::collections::HashSet<String>,
    /// 手动测速覆盖的节点延迟（毫秒）；优先于 /proxies 自带的历史值。
    node_delays: std::collections::HashMap<String, u64>,
    /// 手动测速失败的节点（超时/拒绝）；下次成功测速或停内核时清除。
    node_delay_failures: std::collections::HashSet<String>,
    /// 订阅/导入配置的元数据；内容按 id 存放在 profiles 目录。
    profiles: Vec<ProfileMeta>,
    /// 当前激活的配置 id；None 表示使用内置默认配置。
    active_profile: Option<String>,
    /// 本地基线；controller 地址与 secret 的唯一来源。
    baseline: Option<crate::mihomo::config::LocalBaseline>,
    /// 配置页内联表单。
    profile_form_open: bool,
    profile_form_name: Entity<TextInput>,
    profile_form_url: Entity<TextInput>,
    /// 行内链接编辑使用独立输入实体，避免覆盖添加表单尚未提交的链接。
    profile_edit_url: Entity<TextInput>,
    /// 配置页后台任务忙态提示；非空时禁用相关操作。
    profile_busy: Option<String>,
    profile_error: Option<String>,
    /// 正在自动更新的配置 id；与手动操作（profile_busy）互斥。
    /// 自动更新静默执行，不占用 profile_busy 以免打扰界面。
    auto_update_in_flight: Option<String>,
    /// 编辑弹窗里的自定义 User-Agent 输入框。
    profile_edit_user_agent: Entity<TextInput>,
    /// 行内编辑链接与更新间隔的配置下标；None 表示未在编辑。
    editing_profile_index: Option<usize>,
    /// 右键打开的订阅操作菜单所指向的配置 id；None 表示菜单关闭。
    profile_menu_for: Option<String>,
    /// 行内编辑自动更新间隔的输入框。
    profile_form_interval: Entity<TextInput>,
    /// 登录自启、系统代理与 TUN 的操作失败提示，展示在设置页与概览页。
    integration_error: Option<String>,
    /// 当前已安装 Geo 数据的官方提交与更新时间。
    geodata_info: GeodataInfo,
    /// 设置页手动更新 Geo 数据的后台忙态。
    geodata_updating: bool,
    /// 最近一次 Geo 数据更新结果，仅在设置页展示。
    geodata_status: Option<SharedString>,
    /// 代理页数据拉取中。
    proxies_loading: bool,
    /// 节点展示是否按延迟升序；默认保持内核返回的配置顺序，仅影响当前会话界面。
    node_sort_by_delay: bool,
    /// 分组手动折叠状态（组名 → 是否展开）；未记录的组按节点数自动决定。
    group_expanded: std::collections::HashMap<String, bool>,
    /// 分组已翻页渲染的节点数（组名 → 数量）；未记录的组只渲染首页。
    group_page: std::collections::HashMap<String, usize>,
    /// 节点接入信息（协议/服务器/端口），来自 runtime.yaml，按节点名索引。
    node_endpoints: std::collections::HashMap<String, NodeEndpoint>,
    /// 概览页服务器地址是否明文展示；默认打码，眼睛图标切换。
    server_visible: bool,
    /// 经内核出站检测到的公网出口 IP;未检测为 None。
    net_check_public: Option<String>,
    /// 出口 IP 对应的国家国旗(区域指示符 emoji);未知国家为空串。
    net_check_flag: String,
    /// 本机默认路由出口地址;启动时同步取一次,零网络成本。
    net_check_local: Option<String>,
    /// 出口 IP 查询进行中;按钮据此显示忙态。
    net_check_loading: bool,
    /// 网络检测的失败提示;None 表示无错误。
    net_check_error: Option<String>,
    /// 本次内核会话是否已自动跑过网络检测；换内核后归零。
    net_check_auto_done: bool,
    /// 最近一次网络检测的完整结果；None 表示还没测过。
    net_report: Option<crate::app::net_check::NetReport>,
    /// 网络详情弹层是否展开。
    net_dialog_open: bool,
    /// 正在编辑的附加规则 id；None 表示编辑器处于新增模式。
    editing_extra_rule_id: Option<String>,
    /// 关于页检查更新是否进行中。
    update_checking: bool,
    /// 检查发现新版本时置位；界面用它强调更新提示。
    update_available: bool,
    /// 关于页检查更新的结果文案；None 表示尚未检查。
    update_status: Option<SharedString>,
    /// 内核启动时刻；概览页据此计算运行时长。
    core_started_at: Option<std::time::Instant>,
    /// 本次会话的下载峰值速率（字节/秒）；内核重启时归零。
    peak_down_speed: u64,
    /// 本次会话的上传峰值速率（字节/秒）；内核重启时归零。
    peak_up_speed: u64,
    /// 公网出口的运营商/组织名；来自出口 IP 归属查询，未检测时为 None。
    public_isp: Option<String>,
    /// 应用进程常驻内存（字节）；采样失败为 None。
    app_memory: Option<u64>,
    /// 内核进程常驻内存（字节）；内核未运行或采样失败为 None。
    core_memory: Option<u64>,
    /// 关于页检查更新的结果是否为新版本（用于强调提示）。
    update_status_is_release: bool,
    /// 启动后是否已自动检查过一次更新。
    startup_update_checked: bool,
    /// 策略组展示顺序（按名字），来自 runtime.yaml；未加载时为 None。
    proxy_group_order: Option<Vec<String>>,
    /// 订阅直连检测是否已完成。
    subscription_direct_ready: bool,
}

impl PureClash {
    pub(crate) fn new(
        loaded_config: LoadedConfig,
        startup_mode: StartupMode,
        cx: &mut Context<Self>,
    ) -> Self {
        let LoadedConfig {
            mut config,
            paths,
            geodata_info,
        } = loaded_config;
        // 启动时以 AppConfig 指定的版本确定当前内核路径和可用状态。
        let kernel_available = is_available(&paths, &config.mihomo_version);
        log_info!(
            "app",
            "Pure Clash v{} 启动（{}，内核 {}，{}）",
            Self::CURRENT_VERSION,
            if startup_mode.is_autostart() {
                "登录自启"
            } else {
                "交互启动"
            },
            config.mihomo_version,
            std::env::consts::OS,
        );

        // 本地基线提供 controller 地址与 secret；失败时仅禁用在线功能。
        let mut baseline = match ensure_baseline(&paths) {
            Ok(baseline) => Some(baseline),
            Err(error) => {
                log_error!("core", "初始化本地基线失败：{error:#}");
                None
            }
        };
        // 强制进程名开关以 app.json 为唯一事实来源：启动时把开关对齐到基线，
        // 后续 runtime 生成的 `find-process-mode` 便与界面状态一致。
        if let Some(baseline) = baseline.as_mut()
            && baseline.find_process_always != config.find_process_always
        {
            baseline.find_process_always = config.find_process_always;
            if let Err(error) = save_baseline(&paths, baseline) {
                log_error!("core", "同步进程匹配开关到本地基线失败：{error:#}");
            }
        }
        // 全局附加规则同样以 app.json 为唯一事实来源：启动时把已启用的规则对齐到
        // 基线，再由 merge_runtime 前置到内核规则之前。
        if let Some(baseline) = baseline.as_mut() {
            let enabled = rules_ops::enabled_extra_rule_texts(&config.extra_rules);
            if baseline.extra_rules != enabled {
                baseline.extra_rules = enabled;
                if let Err(error) = save_baseline(&paths, baseline) {
                    log_error!("core", "同步全局附加规则到本地基线失败：{error:#}");
                }
            }
        }

        // 校验持久化的激活配置：内容缺失时回退到内置默认配置。
        if let Some(active_id) = config.active_profile.clone()
            && !profile::profile_yaml_path(&paths, &active_id).is_file()
        {
            config.active_profile = None;
        }
        let active_profile = config.active_profile.clone();
        let runtime_error = profile::sync_runtime_file(
            &paths,
            baseline.as_ref(),
            &config.mihomo_version,
            active_profile.as_deref(),
        )
        .err()
        .map(|error| {
            log_error!("core", "同步运行时配置失败：{error:#}");
            concise_error(&format!("{error:#}"), 200)
        });
        let runtime_ready = runtime_error.is_none();

        // 上次异常退出可能遗留系统代理托管，启动时按记录恢复用户原有设置。
        if let Some(snapshot) = load_system_proxy_state(&paths) {
            match restore_system_proxy(&snapshot) {
                Ok(()) => {
                    clear_system_proxy_state(&paths);
                    log_info!(
                        "proxy",
                        "检测到上次托管的系统代理，启动时已恢复用户原有设置"
                    );
                }
                Err(error) => log_error!("proxy", "启动自愈恢复系统代理设置失败：{error:#}"),
            }
        }

        let extra_rule_payload =
            cx.new(|cx| TextInput::new(t!("rules.extra_payload_placeholder"), cx));
        // 输入变化只触发重绘；识别结果在渲染时现算，不额外存状态。
        let _extra_rule_input_observer = cx.observe(&extra_rule_payload, |this, _, cx| {
            this.sync_extra_rule_draft(cx)
        });
        let profile_form_name = cx.new(|cx| TextInput::new(t!("profiles.name_placeholder"), cx));
        let profile_form_url = cx.new(|cx| TextInput::new(t!("profiles.url_placeholder"), cx));
        let profile_edit_url = cx.new(|cx| TextInput::new(t!("profiles.url_placeholder"), cx));
            let profile_edit_user_agent =
                cx.new(|cx| TextInput::new(t!("profiles.ua_placeholder"), cx));
        let profile_form_interval =
            cx.new(|cx| TextInput::new(t!("profiles.interval_placeholder"), cx));
        let profiles = config.profiles.clone();
        let (autostart_enabled, autostart_available) = match autostart_status() {
            Ok(AutoStartStatus::Enabled) => (true, true),
            Ok(AutoStartStatus::Disabled) => (false, true),
            Ok(AutoStartStatus::Unavailable) => (false, false),
            Err(error) => {
                // 读取失败不代表平台不支持，保留可操作开关让用户可以直接重试写入。
                log_warn!("autostart", "读取登录自启状态失败：{error:#}");
                (false, true)
            }
        };
        let tun_configured = baseline
            .as_ref()
            .is_some_and(|baseline| baseline.tun_enable);

        let mut app = Self {
            page: Page::Overview,
            config,
            paths,
            kernel_available,
            mihomo_process: None,
            core_state: CoreState::Stopped,
            core_start_generation: 0,
            core_restart_pending: false,
            mihomo_error: None,
            system_proxy: false,
            // 配置期望与真实运行状态分离；只有 controller 确认后才显示 TUN 开启。
            tun_configured,
            tun_state: SwitchState::Off,
            autostart_enabled,
            autostart_available,
            // Windows 登录恢复 TUN 必须允许 UAC；Linux 已安装服务可在 false 时静默
            // 启动，服务不可用则回退普通内核，不在登录阶段弹 polkit。
            interactive_elevation_allowed: startup_allows_interactive_elevation(startup_mode),
            // controller 尚未就绪时也展示上次保存的选择。
            mode: ProxyMode::from_controller(baseline.as_ref().map_or(Mode::Rule, |b| b.mode)),
            mode_switching: false,
            node_switching: false,
            proxy_error: None,
            groups: Vec::new(),
            runtime_state_failing: false,
            connections_failing: false,
            connections: Vec::new(),
            traffic_down_speed: 0,
            traffic_up_speed: 0,
            download_total: 0,
            upload_total: 0,
            rules: Vec::new(),
            rules_loading: false,
            rules_error: None,
            rules_failing: false,
            rules_shown: rules::RULES_PAGE_SIZE,
            extra_rule_editor_open: false,
            extra_rule_type: rules_ops::RULE_TYPES[0].to_owned(),
            extra_rule_payload,
            extra_rule_target: String::new(),
            extra_rule_type_picker: false,
            extra_rule_target_picker: false,
            extra_rule_options_shown: extra_rules::OPTION_PAGE_SIZE,
            extra_rule_error: None,
            profiles_view: ProfilesView::Grid,
            usage_refresh_in_flight: false,
            extra_rule_notice: None,
            _extra_rule_input_observer,
            delay_testing: std::collections::HashSet::new(),
            node_delays: std::collections::HashMap::new(),
            node_delay_failures: std::collections::HashSet::new(),
            profiles,
            active_profile,
            baseline,
            profile_form_open: false,
            profile_form_name,
            profile_form_url,
            profile_edit_url,
            profile_edit_user_agent,
            profile_busy: None,
            profile_error: runtime_error,
            auto_update_in_flight: None,
            editing_profile_index: None,
            profile_menu_for: None,
            profile_form_interval,
            integration_error: None,
            geodata_info,
            geodata_updating: false,
            geodata_status: None,
            proxies_loading: false,
            node_sort_by_delay: false,
            group_expanded: std::collections::HashMap::new(),
            group_page: std::collections::HashMap::new(),
            node_endpoints: std::collections::HashMap::new(),
            server_visible: false,
            net_check_public: None,
            net_check_flag: String::new(),
            net_check_local: net_check::local_ip(),
            net_check_loading: false,
            net_check_error: None,
            net_check_auto_done: false,
            net_report: None,
            net_dialog_open: false,
            editing_extra_rule_id: None,
            update_checking: false,
            update_available: false,
            update_status: None,
            core_started_at: None,
            peak_down_speed: 0,
            peak_up_speed: 0,
            public_isp: None,
            app_memory: None,
            core_memory: None,
            update_status_is_release: false,
            startup_update_checked: false,
            proxy_group_order: None,
            subscription_direct_ready: false,
        };
        // 开机即按上次记录的激活配置启动内核；失败时只在界面提示，不阻断打开。
        // 新 runtime 未经校验或未能原子提交时不得沿用磁盘上的旧文件启动。
        if runtime_ready {
            app.start_core(cx);
        }
        app.spawn_connection_poll(cx);
        app.spawn_profile_update_scheduler(cx);
        app
    }
}

impl Render for PureClash {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = self.palette();
        let content = div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .text_color(palette.text)
            .child(render_titlebar(self, palette, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(render_sidebar(self, palette, window, cx))
                    .child(render_page(self, palette, window, cx)),
            )
            // 选择弹窗挂在页面容器之外：遮罩不随页面滚动，弹窗列表也不在页面的
            // 滚动容器里，滚轮不会被外层先消费。
            .when(
                self.extra_rule_type_picker || self.extra_rule_target_picker,
                |root| root.child(render_picker_overlay(self, palette, cx)),
            )
            // 订阅操作菜单同样挂在页面容器之外：遮罩不随页面滚动。
            .when(self.profile_menu_for.is_some(), |root| {
                root.child(render_profile_menu(self, palette, cx))
            })
            // 订阅编辑弹窗同样挂在页面容器之外，两种视图共用。
            .when(self.editing_profile_index.is_some(), |root| {
                root.child(render_profile_editor_overlay(self, palette, cx))
            })
            .when(self.net_dialog_open, |root| {
                root.child(net_check::render_net_dialog(self, palette, cx))
            });

        #[cfg(target_os = "linux")]
        return linux_client_side_decorations(content, palette, window);

        #[cfg(not(target_os = "linux"))]
        content.into_any_element()
    }
}
fn system_proxy_state_file(paths: &AppPaths) -> std::path::PathBuf {
    paths.data_dir.join("system-proxy.json")
}

/// 订阅日志只保留 host（含可选端口），userinfo 与完整 URL 均不进入日志。
fn host_of_url(url: &str) -> &str {
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority)
}

fn save_system_proxy_state(paths: &AppPaths, snapshot: &SystemProxySnapshot) -> anyhow::Result<()> {
    std::fs::create_dir_all(&paths.data_dir)?;
    let bytes = serde_json::to_vec_pretty(snapshot)?;
    let file = system_proxy_state_file(paths);
    let temporary = paths.data_dir.join("system-proxy.json.tmp");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(temporary, file)?;
    Ok(())
}

fn load_system_proxy_state(paths: &AppPaths) -> Option<SystemProxySnapshot> {
    let content = std::fs::read_to_string(system_proxy_state_file(paths)).ok()?;
    serde_json::from_str(&content).ok()
}

fn clear_system_proxy_state(paths: &AppPaths) {
    let _ = std::fs::remove_file(system_proxy_state_file(paths));
}

/// 启用系统代理，捕获并返回托管前的用户设置快照。
fn enable_system_proxy(paths: &AppPaths, server: &str) -> anyhow::Result<()> {
    let snapshot = capture_system_proxy().inspect_err(|error| {
        log_error!("proxy", "捕获用户既有系统代理设置失败：{error:#}");
    })?;
    // 状态必须先于系统设置落盘；应用在 set_system_proxy 后崩溃时，下次启动
    // 仍能恢复用户原配置。
    save_system_proxy_state(paths, &snapshot).inspect_err(|error| {
        log_error!("proxy", "系统代理托管状态文件落盘失败：{error:#}");
    })?;
    if let Err(apply_error) = set_system_proxy(server) {
        log_error!("proxy", "写入系统代理设置失败：{apply_error:#}");
        match restore_system_proxy(&snapshot) {
            Ok(()) => clear_system_proxy_state(paths),
            Err(restore_error) => {
                log_error!(
                    "proxy",
                    "写入失败后自动恢复原系统代理也失败：{restore_error:#}"
                );
                return Err(anyhow::anyhow!(
                    "{apply_error:#}；自动恢复原系统代理也失败：{restore_error:#}"
                ));
            }
        }
        return Err(apply_error);
    }
    Ok(())
}

/// 系统代理操作失败的界面文案。
fn system_proxy_error(error: &anyhow::Error) -> String {
    t!(
        "app.system_proxy_failed",
        error = concise_error(&format!("{error:#}"), 160)
    )
    .into_owned()
}

/// TUN 自动回退时保留可操作的失败原因，但限制长度，避免平台命令输出撑坏横幅。
fn tun_reverted_error(error: &anyhow::Error) -> String {
    t!(
        "app.tun_reverted",
        error = concise_error(&format!("{error:#}"), 180)
    )
    .into_owned()
}

/// 系统代理 / TUN 操作失败的提示横幅。
fn order_groups(groups: &mut [GroupSnapshot], order: &[String]) {
    groups.sort_by_key(|group| {
        if group.name == "GLOBAL" {
            (0usize, 0usize)
        } else {
            let position = order
                .iter()
                .position(|name| name == &group.name)
                .unwrap_or(usize::MAX);
            (1usize, position)
        }
    });
}

/// 未手动操作过分组的默认展开规则：单组超过 [`PROXY_AUTO_COLLAPSE_NODES`]
/// 直接折叠；其余组按显示顺序消耗 [`PROXY_AUTO_EXPAND_NODE_BUDGET`] 节点预算，
/// 超出后默认折叠，保证代理页初始渲染量有上界，大订阅不再卡死界面。
fn section_heading(
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    icon_path: &'static str,
    palette: Palette,
) -> AnyElement {
    let title = title.into();
    let detail = detail.into();
    div()
        .flex()
        .items_center()
        .gap_3()
        .child(
            div()
                .size_8()
                .rounded_md()
                .flex()
                .items_center()
                .justify_center()
                .bg(palette.brand_soft)
                .child(icon(icon_path, palette.brand, 16.0)),
        )
        .child(
            div()
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .text_color(palette.text)
                        .child(title),
                )
                .child(
                    div()
                        .mt_1()
                        .text_xs()
                        .text_color(palette.muted)
                        .child(detail),
                ),
        )
        .into_any_element()
}

fn icon(path: &'static str, color: gpui::Rgba, size: f32) -> AnyElement {
    gpui::svg()
        .path(path)
        .size(px(size))
        .flex_none()
        .text_color(color)
        .into_any_element()
}

fn tr(key: &'static str) -> SharedString {
    SharedString::from(t!(key).into_owned())
}

/// 设置页只展示提交前八位，完整 SHA 仍保存在 Geo 数据状态文件中。
fn short_revision(revision: &str) -> String {
    revision.chars().take(8).collect()
}

fn concise_error(error: &str, max_chars: usize) -> String {
    let mut concise: String = error.chars().take(max_chars).collect();
    if error.chars().count() > max_chars {
        concise.push('…');
    }
    concise
}

/// 字节数人性化展示；超过 1KB 保留一位小数。
fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// 实时速度展示：字节数 + `/s` 后缀。
fn format_speed(bytes_per_second: u64) -> String {
    format!("{}/s", format_bytes(bytes_per_second))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_formatting_stays_readable_across_scales() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2.0 KB");
        assert_eq!(format_bytes(2516582), "2.4 MB");
        assert_eq!(format_speed(8_912_896), "8.5 MB/s");
    }

    #[test]
    fn locales_cover_core_navigation_and_pages() {
        assert_eq!(t!("page.overview", locale = "zh-CN"), "概览");
        assert_eq!(t!("page.overview", locale = "en-US"), "Overview");
        assert_eq!(
            t!("connections.detail", locale = "zh-CN"),
            "实时数据来自 Mihomo controller"
        );
        assert_eq!(
            t!("connections.detail", locale = "en-US"),
            "Live data from the Mihomo controller"
        );
        assert_eq!(
            t!("app.tun_reverted", locale = "zh-CN", error = "DNS 未接管"),
            "TUN 启用失败，已自动关闭并重启内核：DNS 未接管"
        );
        // 错误类文案必须使用 rust-i18n 的 `%{name}` 语法，不能把占位符原样展示。
        assert_eq!(
            t!("about.check_failed", locale = "zh-CN", error = "连接超时"),
            "检查更新失败：连接超时"
        );
        assert_eq!(
            t!("about.check_failed", locale = "en-US", error = "timed out"),
            "Update check failed: timed out"
        );
        assert_eq!(
            t!(
                "settings.geodata_version",
                locale = "zh-CN",
                revision = "d2a52e8a",
                updated = "2026-08-31"
            ),
            "版本 d2a52e8a · 更新于 2026-08-31"
        );
        assert_eq!(
            t!(
                "settings.geodata_update_failed",
                locale = "en-US",
                error = "timed out"
            ),
            "Geo data update failed: timed out"
        );
        assert_eq!(
            t!(
                "profiles.import_failed",
                locale = "zh-CN",
                error = "配置格式错误"
            ),
            "导入本地配置失败：配置格式错误"
        );
        assert_eq!(
            t!(
                "profiles.import_failed",
                locale = "en-US",
                error = "invalid config"
            ),
            "Local profile import failed: invalid config"
        );
        assert_eq!(
            t!(
                "app.system_proxy_failed",
                locale = "zh-CN",
                error = "访问被拒绝"
            ),
            "系统代理设置失败：访问被拒绝"
        );
        assert_eq!(
            t!(
                "app.system_proxy_failed",
                locale = "en-US",
                error = "access denied"
            ),
            "Failed to set system proxy: access denied"
        );
    }

    #[test]
    fn tray_tooltips_cover_locales_and_runtime_states() {
        let chinese = t!(
            "tray.tooltip",
            locale = "zh-CN",
            core = "运行中",
            system_proxy = "开启",
            tun = "关闭"
        );
        let english = t!(
            "tray.tooltip",
            locale = "en-US",
            core = "Stopped",
            system_proxy = "Off",
            tun = "On"
        );

        assert_eq!(
            chinese,
            "Pure Clash\n内核：运行中\n系统代理：开启\nTUN：关闭"
        );
        assert_eq!(
            english,
            "Pure Clash\nCore: Stopped\nSystem proxy: Off\nTUN: On"
        );
        // Windows NOTIFYICONDATAW 的 szTip 只容纳 128 个 UTF-16 单元，需预留结尾 NUL。
        assert!(chinese.encode_utf16().count() <= 127);
        assert!(english.encode_utf16().count() <= 127);

        assert_eq!(t!("tray.menu_open", locale = "zh-CN"), "打开 Pure Clash");
        assert_eq!(t!("tray.menu_quit", locale = "zh-CN"), "退出");
        assert_eq!(t!("tray.menu_open", locale = "en-US"), "Open Pure Clash");
        assert_eq!(t!("tray.menu_quit", locale = "en-US"), "Quit");
    }

    #[test]
    fn concise_error_preserves_utf8_boundaries() {
        assert_eq!(concise_error("启动失败", 2), "启动…");
        assert_eq!(concise_error("short", 10), "short");
    }

    #[test]
    fn subscription_log_host_drops_userinfo_and_path() {
        assert_eq!(
            host_of_url("https://user:password@example.com:8443/sub?token=secret"),
            "example.com:8443"
        );
        assert_eq!(host_of_url("https://[::1]:9090/sub"), "[::1]:9090");
    }

    #[test]
    fn tun_status_requires_running_core_and_controller_confirmation() {
        // 授权中、controller 已就绪但网卡仍初始化中都显示过渡态；确认才显示开启。
        assert_eq!(
            tun_visible_state(CoreState::Starting, SwitchState::Starting),
            SwitchState::Starting
        );
        assert_eq!(
            tun_visible_state(CoreState::Running, SwitchState::Starting),
            SwitchState::Starting
        );
        assert_eq!(
            tun_visible_state(CoreState::Running, SwitchState::On),
            SwitchState::On
        );
        assert_eq!(
            tun_visible_state(CoreState::Starting, SwitchState::On),
            SwitchState::Starting
        );
        // 失败回退和手动停止后，即使有过期状态也不得卡在启动中或显示已开启。
        for state in [SwitchState::Off, SwitchState::Starting, SwitchState::On] {
            assert_eq!(
                tun_visible_state(CoreState::Stopped, state),
                SwitchState::Off
            );
        }
        assert_eq!(
            tun_visible_state(CoreState::Running, SwitchState::Off),
            SwitchState::Off
        );
        assert_eq!(t!("status.starting", locale = "zh-CN"), "启动中…");
        assert_eq!(t!("status.starting", locale = "en-US"), "Starting…");
    }

    #[test]
    fn autostart_elevation_policy_matches_platform_service_model() {
        assert!(startup_allows_interactive_elevation(
            StartupMode::Interactive
        ));
        #[cfg(target_os = "windows")]
        assert!(startup_allows_interactive_elevation(StartupMode::Autostart));
        #[cfg(not(target_os = "windows"))]
        assert!(!startup_allows_interactive_elevation(
            StartupMode::Autostart
        ));
    }
}
