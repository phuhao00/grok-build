//! Unity CLI bridge for desktop visualization.
//!
//! Wraps the standalone [Unity CLI](https://unity.com/cn/blog/meet-the-unity-cli)
//! (`unity`) so the agent desktop can detect the binary, run structured
//! commands, and show an observe → act → verify feedback loop.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CMD_TIMEOUT: Duration = Duration::from_secs(45);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(180);
const GUIDE_STEP_GAP: Duration = Duration::from_millis(650);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliStatus {
    Unknown,
    Checking,
    Missing,
    Ready,
    Error,
}

impl CliStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown | Self::Checking => "检测中",
            Self::Missing => "未安装",
            Self::Ready => "已就绪",
            Self::Error => "异常",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopPhase {
    Observe,
    Act,
    Verify,
}

impl LoopPhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Observe => "观察",
            Self::Act => "行动",
            Self::Verify => "验证",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Self::Observe => "读取现场状态：场景、碰撞体、Play Mode",
            Self::Act => "通过 command / eval 热修复，无需域重载",
            Self::Verify => "重进 Play Mode，确认结果并回报 agent",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Observe => Self::Act,
            Self::Act => Self::Verify,
            Self::Verify => Self::Observe,
        }
    }
}

/// Live scene snapshot driven by observe / act / verify steps.
#[derive(Debug, Clone)]
pub struct SceneSnapshot {
    pub player_y: f32,
    pub ground_collider_enabled: bool,
    pub is_playing: bool,
    pub last_eval_result: String,
    pub note: String,
}

impl Default for SceneSnapshot {
    fn default() -> Self {
        Self {
            player_y: 1.0,
            ground_collider_enabled: true,
            is_playing: false,
            last_eval_result: "—".into(),
            note: "尚未观察场景".into(),
        }
    }
}

impl SceneSnapshot {
    pub fn status_line(&self) -> String {
        format!(
            "Player.y={:.1} · GroundCollider={} · Play={}",
            self.player_y,
            if self.ground_collider_enabled {
                "ON"
            } else {
                "OFF"
            },
            if self.is_playing { "ON" } else { "OFF" }
        )
    }
}

#[derive(Debug, Clone)]
pub struct OpRecord {
    pub id: u64,
    pub title: String,
    pub command: String,
    pub phase: LoopPhase,
    pub ok: bool,
    pub summary: String,
    pub detail: String,
    pub at_unix: u64,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineStatus {
    Unknown,
    Checking,
    NotInstalled,
    Installing,
    PendingImport,
    Installed,
    Error,
}

impl PipelineStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "未知",
            Self::Checking => "检查中",
            Self::NotInstalled => "未安装",
            Self::Installing => "安装中",
            Self::PendingImport => "等待编辑器加载",
            Self::Installed => "已安装",
            Self::Error => "异常",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorLinkStatus {
    Unknown,
    Checking,
    Disconnected,
    Connected,
}

impl EditorLinkStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "未知",
            Self::Checking => "探测中",
            Self::Disconnected => "未连接",
            Self::Connected => "已连接",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnityAction {
    RefreshDetect,
    ListEditors,
    ListPipeline,
    InstallPipeline,
    ListCommands,
    ProbeEditor,
    Eval,
    ObserveCollider,
    FixCollider,
    EnterPlayMode,
    ExitPlayMode,
    RunFullLoop,
}

impl UnityAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::RefreshDetect => "重新检测",
            Self::ListEditors => "列出编辑器",
            Self::ListPipeline => "刷新 Pipeline",
            Self::InstallPipeline => "安装 Pipeline",
            Self::ListCommands => "发现命令",
            Self::ProbeEditor => "探测编辑器",
            Self::Eval => "运行 Eval",
            Self::ObserveCollider => "观察碰撞体",
            Self::FixCollider => "修复碰撞体",
            Self::EnterPlayMode => "进入 Play",
            Self::ExitPlayMode => "退出 Play",
            Self::RunFullLoop => "跑完整闭环",
        }
    }
}

pub const EVAL_PRESETS: &[(&str, &str)] = &[
    ("Play?", "return UnityEditor.EditorApplication.isPlaying;"),
    ("Version", "return Application.version;"),
    ("DataPath", "return Application.dataPath;"),
    (
        "Collider",
        "var go = GameObject.Find(\"Ground\"); var c = go != null ? go.GetComponent<Collider>() : null; return c != null && c.enabled;",
    ),
];

const CREATE_SPHERE_EVAL: &str = "var go = GameObject.Find(\"BonySphere\"); if (go == null) { go = GameObject.CreatePrimitive(PrimitiveType.Sphere); go.name = \"BonySphere\"; } go.transform.position = Vector3.zero; UnityEditor.Selection.activeGameObject = go; UnityEditor.SceneManagement.EditorSceneManager.MarkSceneDirty(go.scene); return go.name;";

/// Chat → local Unity CLI (bypasses the coding agent).
#[derive(Debug, Clone, Copy)]
pub struct UnityChatCmd {
    pub chip: &'static str,
    pub slash: &'static str,
    pub action: UnityAction,
    pub eval: Option<&'static str>,
    /// Compact phrases (no spaces); matched with `contains` after normalize.
    pub phrases: &'static [&'static str],
}

/// Primary chips shown in the chat composer / empty state.
pub const UNITY_CHAT_CHIPS: &[UnityChatCmd] = &[
    UnityChatCmd {
        chip: "创建球体",
        slash: "/unity sphere",
        action: UnityAction::Eval,
        eval: Some(CREATE_SPHERE_EVAL),
        phrases: &[
            "创建球体",
            "新建球体",
            "生成球体",
            "画一个球体",
            "画个球体",
            "场景画一个球体",
            "场景里放一个球体",
            "unitysphere",
        ],
    },
    UnityChatCmd {
        chip: "探测编辑器",
        slash: "/unity probe",
        action: UnityAction::ProbeEditor,
        eval: None,
        phrases: &["探测编辑器", "检查编辑器", "连接编辑器", "unityprobe"],
    },
    UnityChatCmd {
        chip: "进入 Play",
        slash: "/unity play",
        action: UnityAction::EnterPlayMode,
        eval: None,
        phrases: &["进入play", "开始播放", "开始play", "unityplay"],
    },
    UnityChatCmd {
        chip: "退出 Play",
        slash: "/unity stop",
        action: UnityAction::ExitPlayMode,
        eval: None,
        phrases: &["退出play", "停止播放", "停止play", "unitystop"],
    },
    UnityChatCmd {
        chip: "跑闭环",
        slash: "/unity loop",
        action: UnityAction::RunFullLoop,
        eval: None,
        phrases: &["运行完整闭环", "跑完整闭环", "完整闭环", "unityloop"],
    },
    UnityChatCmd {
        chip: "查版本",
        slash: "/unity version",
        action: UnityAction::Eval,
        eval: Some(EVAL_PRESETS[1].1),
        phrases: &["查询unity版本", "查unity版本", "unity版本", "unityversion"],
    },
    UnityChatCmd {
        chip: "安装 Pipeline",
        slash: "/unity install",
        action: UnityAction::InstallPipeline,
        eval: None,
        phrases: &[
            "安装pipeline",
            "装pipeline",
            "unityinstall",
            "unitypipelineinstall",
        ],
    },
];

/// Extra slash / phrase matches not shown as chips.
pub const UNITY_CHAT_EXTRA: &[UnityChatCmd] = &[
    UnityChatCmd {
        chip: "检测 CLI",
        slash: "/unity detect",
        action: UnityAction::RefreshDetect,
        eval: None,
        phrases: &["检测unity", "重新检测unity", "检测cli", "unitydetect"],
    },
    UnityChatCmd {
        chip: "刷新 Pipeline",
        slash: "/unity pipeline",
        action: UnityAction::ListPipeline,
        eval: None,
        phrases: &["刷新pipeline", "检查pipeline", "unitypipeline"],
    },
    UnityChatCmd {
        chip: "发现命令",
        slash: "/unity commands",
        action: UnityAction::ListCommands,
        eval: None,
        phrases: &["发现命令", "列出unity命令", "unitycommands"],
    },
    UnityChatCmd {
        chip: "查项目路径",
        slash: "/unity path",
        action: UnityAction::Eval,
        eval: Some(EVAL_PRESETS[2].1),
        phrases: &["查询项目路径", "查项目路径", "unitypath"],
    },
    UnityChatCmd {
        chip: "查碰撞体",
        slash: "/unity collider",
        action: UnityAction::Eval,
        eval: Some(EVAL_PRESETS[3].1),
        phrases: &["查询碰撞体", "查碰撞体", "unitycollider"],
    },
    UnityChatCmd {
        chip: "查播放状态",
        slash: "/unity playing",
        action: UnityAction::Eval,
        eval: Some(EVAL_PRESETS[0].1),
        phrases: &["查询播放状态", "查播放状态", "是否在play", "unityplaying"],
    },
];

pub fn normalize_unity_chat(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

pub fn parse_unity_chat_command(text: &str) -> Option<&'static UnityChatCmd> {
    let n = normalize_unity_chat(text);
    if n.is_empty() {
        return None;
    }
    for cmd in UNITY_CHAT_CHIPS.iter().chain(UNITY_CHAT_EXTRA.iter()) {
        let slash_key = normalize_unity_chat(cmd.slash);
        if n == slash_key {
            return Some(cmd);
        }
        for p in cmd.phrases {
            if n == *p || n.contains(p) {
                return Some(cmd);
            }
        }
    }
    None
}

/// Compiles parameterized natural-language creation requests into a bounded
/// Unity Eval operation. This is intentionally data-driven (count + primitive)
/// rather than a growing list of exact phrases.
pub fn compile_unity_scene_command(text: &str) -> Option<(String, String)> {
    let n = normalize_unity_chat(text);
    if n.contains("删除选中") || n.contains("删除这些") || n.contains("移除选中") {
        return Some((
            "删除选中对象".into(),
            "var targets = UnityEditor.Selection.gameObjects; if (targets == null || targets.Length == 0) return \"No selected objects\"; int count = targets.Length; foreach (var target in targets) UnityEditor.Undo.DestroyObjectImmediate(target); return \"Deleted \" + count + \" objects\";".into(),
        ));
    }
    if n.contains("复制选中") || n.contains("复制这些") || n.contains("再复制") {
        return Some((
            "复制选中对象".into(),
            "var targets = UnityEditor.Selection.gameObjects; if (targets == null || targets.Length == 0) return \"No selected objects\"; var created = new System.Collections.Generic.List<GameObject>(); foreach (var target in targets) { var copy = UnityEngine.Object.Instantiate(target); copy.name = target.name + \" Copy\"; copy.transform.position += Vector3.right * 2f; UnityEditor.Undo.RegisterCreatedObjectUndo(copy, \"Duplicate object\"); created.Add(copy); UnityEditor.SceneManagement.EditorSceneManager.MarkSceneDirty(copy.scene); } UnityEditor.Selection.objects = created.ToArray(); return \"Duplicated \" + created.Count + \" objects\";".into(),
        ));
    }
    if n.contains("放大") || n.contains("缩小") {
        let factor = if n.contains("缩小") { "0.5f" } else { "2f" };
        return Some((
            if n.contains("缩小") {
                "缩小选中对象"
            } else {
                "放大选中对象"
            }
            .into(),
            format!(
                "var targets = UnityEditor.Selection.gameObjects; if (targets == null || targets.Length == 0) return \"No selected objects\"; foreach (var target in targets) {{ UnityEditor.Undo.RecordObject(target.transform, \"Scale object\"); target.transform.localScale *= {factor}; UnityEditor.SceneManagement.EditorSceneManager.MarkSceneDirty(target.scene); }} return \"Scaled \" + targets.Length + \" objects\";"
            ),
        ));
    }
    let movement = if n.contains("向上") || n.contains("往上") {
        Some(("向上移动选中对象", "Vector3.up"))
    } else if n.contains("向下") || n.contains("往下") {
        Some(("向下移动选中对象", "Vector3.down"))
    } else if n.contains("向左") || n.contains("往左") {
        Some(("向左移动选中对象", "Vector3.left"))
    } else if n.contains("向右") || n.contains("往右") {
        Some(("向右移动选中对象", "Vector3.right"))
    } else if n.contains("向前") || n.contains("往前") {
        Some(("向前移动选中对象", "Vector3.forward"))
    } else if n.contains("向后") || n.contains("往后") {
        Some(("向后移动选中对象", "Vector3.back"))
    } else {
        None
    };
    if let Some((label, direction)) = movement {
        return Some((
            label.into(),
            format!(
                "var targets = UnityEditor.Selection.gameObjects; if (targets == null || targets.Length == 0) return \"No selected objects\"; foreach (var target in targets) {{ UnityEditor.Undo.RecordObject(target.transform, \"Move object\"); target.transform.position += {direction}; UnityEditor.SceneManagement.EditorSceneManager.MarkSceneDirty(target.scene); }} return \"Moved \" + targets.Length + \" objects\";"
            ),
        ));
    }
    if n.contains("刚体") || n.contains("rigidbody") {
        return Some((
            "给选中对象添加刚体".into(),
            "var targets = UnityEditor.Selection.gameObjects; if (targets == null || targets.Length == 0) return \"No selected objects\"; int changed = 0; foreach (var target in targets) { foreach (var t in target.GetComponentsInChildren<Transform>(true)) { if (t.GetComponent<Rigidbody>() == null) { UnityEditor.Undo.AddComponent<Rigidbody>(t.gameObject); changed++; } } UnityEditor.SceneManagement.EditorSceneManager.MarkSceneDirty(target.scene); } return \"Added Rigidbody to \" + changed + \" objects\";".into(),
        ));
    }
    let color = [
        ("绿色", "green", "Color.green"),
        ("红色", "red", "Color.red"),
        ("蓝色", "blue", "Color.blue"),
        ("黄色", "yellow", "Color.yellow"),
        ("白色", "white", "Color.white"),
        ("黑色", "black", "Color.black"),
        ("灰色", "gray", "Color.gray"),
        ("青色", "cyan", "Color.cyan"),
        ("紫色", "magenta", "Color.magenta"),
    ]
    .iter()
    .find(|(cn, en, _)| n.contains(cn) || n.contains(en));
    if let Some((cn, _, unity_color)) = color {
        let eval = format!(
            "var targets = UnityEditor.Selection.gameObjects; if (targets == null || targets.Length == 0) return \"No selected objects\"; int changed = 0; var shader = Shader.Find(\"Universal Render Pipeline/Lit\") ?? Shader.Find(\"Standard\"); foreach (var target in targets) {{ foreach (var renderer in target.GetComponentsInChildren<Renderer>(true)) {{ UnityEditor.Undo.RecordObject(renderer, \"Set color\"); var material = new Material(shader); material.name = \"Bony {cn}\"; material.color = {unity_color}; renderer.sharedMaterial = material; changed++; }} UnityEditor.SceneManagement.EditorSceneManager.MarkSceneDirty(target.scene); }} return \"Colored \" + changed + \" renderers\";"
        );
        return Some((format!("把选中对象设为{cn}"), eval));
    }
    if !["创建", "新建", "生成", "放", "画"]
        .iter()
        .any(|verb| n.contains(verb))
    {
        return None;
    }
    let (cn_name, primitive) = if n.contains("球体") || n.contains("sphere") {
        ("球体", "Sphere")
    } else if n.contains("立方体")
        || n.contains("正方体")
        || n.contains("方块")
        || n.contains("cube")
    {
        ("立方体", "Cube")
    } else if n.contains("胶囊") || n.contains("capsule") {
        ("胶囊体", "Capsule")
    } else if n.contains("圆柱") || n.contains("cylinder") {
        ("圆柱体", "Cylinder")
    } else if n.contains("平面") || n.contains("plane") {
        ("平面", "Plane")
    } else {
        return None;
    };
    let ascii_count = n
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse::<usize>()
        .ok();
    let cn_count = [
        ("十", 10),
        ("九", 9),
        ("八", 8),
        ("七", 7),
        ("六", 6),
        ("五", 5),
        ("四", 4),
        ("三", 3),
        ("二", 2),
        ("两", 2),
        ("一", 1),
    ]
    .iter()
    .find_map(|(token, value)| n.contains(token).then_some(*value));
    let count = ascii_count.or(cn_count).unwrap_or(1).clamp(1, 50);
    let prefix = format!("Bony{primitive}");
    let eval = format!(
        "var root = new GameObject(\"{prefix}Group\"); UnityEditor.Undo.RegisterCreatedObjectUndo(root, \"Create {count} {primitive}\"); int columns = Mathf.CeilToInt(Mathf.Sqrt({count})); for (int i = 0; i < {count}; i++) {{ var go = GameObject.CreatePrimitive(PrimitiveType.{primitive}); go.name = \"{prefix}_\" + (i + 1); go.transform.SetParent(root.transform); float x = (i % columns) * 2f - (columns - 1) * 1f; float z = (i / columns) * 2f; go.transform.position = new Vector3(x, 0f, z); }} UnityEditor.Selection.activeGameObject = root; UnityEditor.SceneManagement.EditorSceneManager.MarkSceneDirty(root.scene); if (UnityEditor.SceneView.lastActiveSceneView != null) UnityEditor.SceneView.lastActiveSceneView.FrameSelected(); return \"Created {count} {primitive}\";"
    );
    Some((format!("创建 {count} 个{cn_name}"), eval))
}

#[derive(serde::Deserialize)]
struct GeneratedUnityPlan {
    summary: String,
    csharp: String,
}

/// Parse the agent's generic Unity plan and reject APIs that can escape the
/// editor/project boundary. UnityEditor/UnityEngine remain available, allowing
/// arbitrary scene, asset, prefab, animation, UI and component operations.
pub fn parse_generated_unity_plan(raw: &str) -> Result<(String, String), String> {
    let (summary, csharp, risks) = parse_generated_unity_plan_unrestricted(raw)?;
    if let Some(api) = risks.first() {
        return Err(format!("Unity 计划包含禁止的越界 API：{api}"));
    }
    Ok((summary, csharp))
}

pub fn parse_generated_unity_plan_unrestricted(
    raw: &str,
) -> Result<(String, String, Vec<String>), String> {
    let trimmed = raw.trim();
    let json = if trimmed.starts_with("```") {
        let body = trimmed
            .split_once('\n')
            .map(|(_, body)| body)
            .unwrap_or(trimmed);
        body.rsplit_once("```")
            .map(|(body, _)| body)
            .unwrap_or(body)
    } else {
        trimmed
    };
    let plan: GeneratedUnityPlan = serde_json::from_str(json.trim())
        .map_err(|error| format!("Unity 计划格式无效：{error}"))?;
    if plan.summary.trim().is_empty() || plan.csharp.trim().is_empty() {
        return Err("Unity 计划缺少 summary 或 csharp".into());
    }
    if plan.csharp.len() > 24_000 {
        return Err("Unity 计划过长，已拒绝执行".into());
    }
    let lower = plan.csharp.to_ascii_lowercase();
    let blocked = [
        "system.io",
        "system.net",
        "system.diagnostics.process",
        "microsoft.win32",
        "dllimport",
        "marshal.",
        "environment.exit",
        "file.",
        "directory.",
        "webrequest",
        "httpclient",
        "reflection",
        "assembly.load",
    ];
    let risks = blocked
        .iter()
        .filter(|api| lower.contains(**api))
        .map(|api| (*api).to_string())
        .collect();
    Ok((
        plan.summary.trim().to_string(),
        plan.csharp.trim().to_string(),
        risks,
    ))
}

pub fn unity_chat_help_text() -> String {
    let mut lines = vec![
        "### 对话控制 Unity（本地 CLI，不经 Agent）".to_string(),
        String::new(),
        "在聊天输入框点 **Unity** 打开快捷指令，或直接发送：".to_string(),
        String::new(),
    ];
    lines.push("自然语言场景操作支持数量和基础类型，例如：`创建3个球体`、`生成五个立方体`、`放两个胶囊体`。".into());
    lines.push(String::new());
    lines.push("快捷命令：".into());
    lines.push(String::new());
    for cmd in UNITY_CHAT_CHIPS.iter().chain(UNITY_CHAT_EXTRA.iter()) {
        lines.push(format!("- **{}** · `{}`", cmd.chip, cmd.slash));
    }
    lines.push(String::new());
    lines.push(
        "首次使用请先在侧栏「Unity 控制」选好工程根并完成引导（CLI → Pipeline → 探测）。".into(),
    );
    lines.join("\n")
}

pub fn wants_unity_help(text: &str) -> bool {
    let n = normalize_unity_chat(text);
    matches!(
        n.as_str(),
        "/unity"
            | "/unityhelp"
            | "/unity帮助"
            | "unity帮助"
            | "unity指令"
            | "unity命令"
            | "帮助unity"
    ) || n == "unity?"
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupStep {
    InstallCli,
    DetectCli,
    PickProject,
    InstallPipeline,
    ProbeEditor,
    RunLoop,
}

impl SetupStep {
    pub const ALL: [SetupStep; 6] = [
        Self::InstallCli,
        Self::DetectCli,
        Self::PickProject,
        Self::InstallPipeline,
        Self::ProbeEditor,
        Self::RunLoop,
    ];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::InstallCli => "安装 Unity CLI",
            Self::DetectCli => "检测 CLI",
            Self::PickProject => "确认 Unity 工程根（不要用 agent worktree）",
            Self::InstallPipeline => "安装 Pipeline",
            Self::ProbeEditor => "探测编辑器",
            Self::RunLoop => "跑闭环验证",
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Self::InstallCli => "本机需要独立的 unity 命令行（不是编辑器 Unity.exe）",
            Self::DetectCli => "确认 PATH / UNITY_CLI 能找到 unity 二进制",
            Self::PickProject => "必须是含 Assets + ProjectSettings 的工程根；聊天任务目录无效",
            Self::InstallPipeline => "在项目中执行 unity pipeline install",
            Self::ProbeEditor => "编辑器打开同一工程后，unity command 才能响应",
            Self::RunLoop => "观察 → 行动 → 验证（演示或实机）",
        }
    }

    pub fn primary_label(self) -> &'static str {
        match self {
            Self::InstallCli => "复制安装命令",
            Self::DetectCli => "重新检测",
            Self::PickProject => "选择 Unity 工程…",
            Self::InstallPipeline => "安装 Pipeline",
            Self::ProbeEditor => "探测编辑器",
            Self::RunLoop => "跑完整闭环",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Done,
    Current,
    Locked,
}

#[derive(Debug)]
pub struct UnityState {
    pub status: CliStatus,
    pub cli_path: Option<PathBuf>,
    pub version_line: String,
    pub last_error: Option<String>,
    pub project_path: PathBuf,
    pub editors_json: String,
    pub editors_summary: String,
    pub pipeline_status: PipelineStatus,
    pub pipeline_summary: String,
    pub pipeline_detail: String,
    pub editor_link: EditorLinkStatus,
    pub commands_summary: String,
    pub eval_input: String,
    pub busy: bool,
    pub loop_phase: LoopPhase,
    pub demo_mode: bool,
    pub scene: SceneSnapshot,
    pub guide_label: Option<String>,
    pub toast: Option<String>,
    pub history: Vec<OpRecord>,
    pub next_id: u64,
    /// Active onboarding step highlighted in the wizard.
    pub setup_step: SetupStep,
    /// User can expand earlier/later steps manually.
    pub setup_focus: Option<SetupStep>,
    /// When true, agent cwd must not overwrite the chosen Unity project path.
    pub project_locked: bool,
    pending_rx: Option<mpsc::Receiver<UnityWorkerMsg>>,
    guide_queue: Vec<UnityAction>,
    guide_next_at: Option<Instant>,
}

impl Default for UnityState {
    fn default() -> Self {
        let saved = load_unity_project_pref();
        let project_path = saved
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let mut state = Self {
            status: CliStatus::Unknown,
            cli_path: None,
            version_line: String::new(),
            last_error: None,
            project_path,
            editors_json: String::new(),
            editors_summary: "尚未拉取".into(),
            pipeline_status: PipelineStatus::Unknown,
            pipeline_summary: "尚未检查".into(),
            pipeline_detail: String::new(),
            editor_link: EditorLinkStatus::Unknown,
            commands_summary: "尚未探测".into(),
            eval_input: EVAL_PRESETS[0].1.into(),
            busy: false,
            loop_phase: LoopPhase::Observe,
            demo_mode: false,
            scene: SceneSnapshot::default(),
            guide_label: None,
            toast: None,
            history: Vec::new(),
            next_id: 1,
            setup_step: SetupStep::InstallCli,
            setup_focus: None,
            project_locked: saved.is_some(),
            pending_rx: None,
            guide_queue: Vec::new(),
            guide_next_at: None,
        };
        if let Some(path) = saved {
            if let Some(root) = resolve_unity_project_root(&path) {
                state.project_path = root;
            }
        }
        state.sync_setup_step();
        state
    }
}

#[derive(Debug)]
enum UnityWorkerMsg {
    Detected {
        path: Option<PathBuf>,
        version: String,
        error: Option<String>,
    },
    CommandDone {
        action: UnityAction,
        title: String,
        command: String,
        phase: LoopPhase,
        ok: bool,
        stdout: String,
        stderr: String,
        elapsed_ms: u64,
    },
}

impl UnityState {
    pub fn ensure_detecting(&mut self) {
        if !matches!(self.status, CliStatus::Unknown) || self.busy {
            return;
        }
        self.status = CliStatus::Checking;
        self.busy = true;
        let (tx, rx) = mpsc::channel();
        self.pending_rx = Some(rx);
        thread::spawn(move || {
            let result = detect_cli();
            let _ = tx.send(UnityWorkerMsg::Detected {
                path: result.path,
                version: result.version,
                error: result.error,
            });
        });
    }

    /// Drain worker messages and advance guided demo queue.
    pub fn poll(&mut self) -> bool {
        let mut changed = self.drain_worker();
        changed |= self.tick_guide();
        changed
    }

    fn drain_worker(&mut self) -> bool {
        let Some(rx) = self.pending_rx.as_ref() else {
            return false;
        };
        let mut msgs = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            msgs.push(msg);
        }
        if msgs.is_empty() {
            return false;
        }
        for msg in msgs {
            match msg {
                UnityWorkerMsg::Detected {
                    path,
                    version,
                    error,
                } => {
                    self.busy = false;
                    self.cli_path = path.clone();
                    self.version_line = version;
                    self.last_error = error.clone();
                    if path.is_some() {
                        self.status = CliStatus::Ready;
                        self.demo_mode = false;
                        self.toast = Some("已检测到 Unity CLI".into());
                    } else if error.is_some() {
                        self.status = CliStatus::Error;
                        self.demo_mode = true;
                        self.toast = Some("CLI 异常，已切换演示模式".into());
                    } else {
                        self.status = CliStatus::Missing;
                        self.demo_mode = true;
                        self.toast = Some("未安装 CLI，已切换演示模式".into());
                    }
                    self.sync_setup_step();
                }
                UnityWorkerMsg::CommandDone {
                    action,
                    title,
                    command,
                    phase,
                    ok,
                    stdout,
                    stderr,
                    elapsed_ms,
                } => {
                    self.busy = false;
                    let detail = merge_streams(&stdout, &stderr);
                    let summary = if ok {
                        match action {
                            UnityAction::ListEditors => summarize_editors_json(&stdout),
                            UnityAction::Eval => summarize_eval_output(&stdout),
                            _ => truncate_one_line(&stdout, 120),
                        }
                    } else {
                        truncate_one_line(if stderr.is_empty() { &stdout } else { &stderr }, 120)
                    };
                    self.push_record(OpRecord {
                        id: self.next_id,
                        title,
                        command,
                        phase,
                        ok,
                        summary,
                        detail,
                        at_unix: now_unix(),
                        elapsed_ms,
                    });
                    self.next_id += 1;
                    if ok {
                        self.apply_action_effects(action, phase, &stdout);
                        // Scene-driven actions already set the highlight phase.
                        if !matches!(
                            action,
                            UnityAction::ObserveCollider
                                | UnityAction::FixCollider
                                | UnityAction::EnterPlayMode
                        ) {
                            self.loop_phase = phase.next();
                        }
                    } else {
                        self.apply_action_failure(action, &stderr, &stdout);
                    }
                    self.sync_setup_step();
                    if self.guide_queue.is_empty() {
                        if self.guide_label.is_some() {
                            self.toast = Some("完整闭环完成 ✓".into());
                        }
                        self.guide_label = None;
                    } else {
                        self.guide_next_at = Some(Instant::now() + GUIDE_STEP_GAP);
                    }
                }
            }
        }
        if !self.busy {
            self.pending_rx = None;
        }
        true
    }

    fn tick_guide(&mut self) -> bool {
        if self.busy || self.guide_queue.is_empty() {
            return false;
        }
        if let Some(at) = self.guide_next_at {
            if Instant::now() < at {
                return false;
            }
        }
        let next = self.guide_queue.remove(0);
        self.guide_label = Some(format!(
            "闭环 {}/{} · {}",
            self.guide_step_index(),
            self.guide_total_steps(),
            next.label()
        ));
        self.guide_next_at = None;
        self.run_action(next);
        true
    }

    fn guide_total_steps(&self) -> usize {
        // Full loop is always 3 steps; remaining + 1 currently starting.
        self.guide_queue.len() + 1
    }

    fn guide_step_index(&self) -> usize {
        3usize.saturating_sub(self.guide_queue.len())
    }

    fn apply_action_effects(&mut self, action: UnityAction, phase: LoopPhase, stdout: &str) {
        let trimmed = stdout.trim();
        match action {
            UnityAction::ListEditors => {
                self.editors_json = trimmed.to_string();
                self.editors_summary = summarize_editors_json(trimmed);
            }
            UnityAction::ListPipeline => {
                self.pipeline_detail = trimmed.to_string();
                self.pipeline_summary = summarize_pipeline_list(trimmed);
                self.pipeline_status = infer_pipeline_status(trimmed, true);
                // The list response already contains the editor server state.
                // Use it directly; preview CLI table columns are not fixed.
                if let Some(reachable) = pipeline_server_reachable(trimmed) {
                    self.editor_link = if reachable {
                        EditorLinkStatus::Connected
                    } else {
                        EditorLinkStatus::Disconnected
                    };
                    self.commands_summary = if reachable {
                        "Pipeline 服务可达，可以执行 command / eval".into()
                    } else {
                        "Pipeline 包已登记，但编辑器服务不可达".into()
                    };
                }
                if pipeline_declared(&self.project_path)
                    && !pipeline_loaded_by_editor(&self.project_path)
                {
                    self.pipeline_status = PipelineStatus::PendingImport;
                    self.pipeline_summary =
                        "已写入 manifest，但当前 Editor 尚未解析；请完全关闭并重新打开工程".into();
                }
            }
            UnityAction::InstallPipeline => {
                self.pipeline_detail = trimmed.to_string();
                self.pipeline_summary = truncate_one_line(trimmed, 160);
                self.pipeline_status = if trimmed.is_empty() {
                    PipelineStatus::Installed
                } else {
                    infer_pipeline_status(trimmed, true)
                };
                if self.pipeline_status == PipelineStatus::Installed {
                    self.toast = Some("Pipeline 已安装，请等编辑器重编译后再探测".into());
                }
                if pipeline_declared(&self.project_path)
                    && !pipeline_loaded_by_editor(&self.project_path)
                {
                    self.pipeline_status = PipelineStatus::PendingImport;
                    self.pipeline_summary =
                        "已写入 manifest；请完全关闭并重新打开 Unity 工程".into();
                }
            }
            UnityAction::ListCommands | UnityAction::ProbeEditor => {
                self.commands_summary = truncate_one_line(trimmed, 160);
                self.editor_link = infer_editor_link(trimmed, true);
                // Only infer Pipeline from a real editor link, not demo chatter.
                if !self.demo_mode
                    && self.editor_link == EditorLinkStatus::Connected
                    && self.pipeline_status != PipelineStatus::Installed
                {
                    self.pipeline_status = PipelineStatus::Installed;
                    self.pipeline_summary = "编辑器已响应 command（视为已安装）".into();
                }
            }
            UnityAction::ObserveCollider => {
                self.scene.ground_collider_enabled = false;
                self.scene.is_playing = true;
                self.scene.player_y = -2.4;
                self.scene.last_eval_result = "false".into();
                self.scene.note = "观察：GroundCollider 被禁用，玩家掉出地板".into();
                self.loop_phase = LoopPhase::Act;
            }
            UnityAction::FixCollider => {
                self.scene.ground_collider_enabled = true;
                self.scene.last_eval_result = "true".into();
                self.scene.note = "行动：已重新启用 GroundCollider".into();
                self.loop_phase = LoopPhase::Verify;
            }
            UnityAction::EnterPlayMode => {
                self.scene.is_playing = true;
                if self.scene.ground_collider_enabled {
                    self.scene.player_y = 1.0;
                    self.scene.note = "验证：Play Mode 中玩家站在地板上".into();
                } else {
                    self.scene.player_y = -2.4;
                    self.scene.note = "验证：碰撞体仍关闭，玩家掉落".into();
                }
                self.scene.last_eval_result = "true".into();
            }
            UnityAction::ExitPlayMode => {
                self.scene.is_playing = false;
                self.scene.note = "已退出 Play Mode".into();
                self.scene.last_eval_result = "false".into();
            }
            UnityAction::Eval => {
                self.scene.last_eval_result = truncate_one_line(trimmed, 80);
                self.scene.note = format!("Eval 完成（{}）", phase.label());
                if !trimmed.is_empty() && !trimmed.to_lowercase().contains("error") {
                    self.editor_link = EditorLinkStatus::Connected;
                }
            }
            UnityAction::RefreshDetect | UnityAction::RunFullLoop => {}
        }
    }

    fn apply_action_failure(&mut self, action: UnityAction, stderr: &str, stdout: &str) {
        let msg = truncate_one_line(
            if stderr.trim().is_empty() {
                stdout
            } else {
                stderr
            },
            160,
        );
        match action {
            UnityAction::InstallPipeline => {
                self.pipeline_status = PipelineStatus::Error;
                self.pipeline_summary = msg.clone();
                self.pipeline_detail = merge_streams(stdout, stderr);
                self.toast = Some(format!("Pipeline 安装失败：{msg}"));
            }
            UnityAction::ListPipeline => {
                self.pipeline_status = PipelineStatus::Error;
                self.pipeline_summary = msg;
            }
            UnityAction::ListCommands | UnityAction::ProbeEditor | UnityAction::Eval => {
                self.editor_link = EditorLinkStatus::Disconnected;
                self.commands_summary = msg.clone();
                if msg.to_lowercase().contains("pipeline") {
                    self.toast = Some("编辑器未响应：请先安装 Pipeline 并打开项目".into());
                }
            }
            _ => {}
        }
    }

    fn push_record(&mut self, record: OpRecord) {
        self.history.insert(0, record);
        if self.history.len() > 40 {
            self.history.truncate(40);
        }
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
        self.toast = Some("已清空操作时间线".into());
    }

    pub fn latest_record_id(&self) -> u64 {
        self.history.first().map(|record| record.id).unwrap_or(0)
    }

    pub fn latest_chat_result_since(&self, previous_id: u64) -> String {
        match self
            .history
            .first()
            .filter(|record| record.id != previous_id)
        {
            Some(record) if record.ok => format!(
                "Unity 操作已完成：{}。{}（{} ms）",
                record.title, record.summary, record.elapsed_ms
            ),
            Some(record) => format!("Unity 操作失败：{}。{}", record.title, record.summary),
            None => format!(
                "Unity 状态已刷新：CLI {}，Pipeline {}，编辑器{}。",
                self.status.label(),
                self.pipeline_status.label(),
                self.editor_link.label()
            ),
        }
    }

    pub fn reset_scene(&mut self) {
        self.scene = SceneSnapshot::default();
        self.loop_phase = LoopPhase::Observe;
        self.guide_queue.clear();
        self.guide_label = None;
        self.toast = Some("场景快照已重置".into());
    }

    pub fn chat_briefing(&self) -> String {
        let mode = if self.demo_mode { "演示" } else { "实机" };
        let latest = self
            .history
            .first()
            .map(|r| format!("{} · {} · {}", r.phase.label(), r.title, r.summary))
            .unwrap_or_else(|| "尚无操作记录".into());
        let project_ok = looks_like_unity_project(&self.project_path);

        // NEVER instruct the coding agent to shell out to `unity`.
        // Missing CLI, wrong cwd (task worktrees), or long pipeline installs all hang the UI.
        format!(
            "【只读分析，禁止执行任何 unity / pipeline / command eval 终端命令】\n\
             \n\
             当前 Unity 面板状态（{mode}）：\n\
             - CLI: {}\n\
             - 绑定工程: {}{}\n\
             - Pipeline: {} · {}\n\
             - 编辑器连接: {}\n\
             - 阶段: {}\n\
             - 场景: {}\n\
             - 备注: {}\n\
             - 最近面板操作: {}\n\
             \n\
             请只向用户解释现状与下一步；所有安装/探测/闭环必须让用户在侧栏「Unity CLI」引导页点按钮完成，\
             不要使用 run_terminal_cmd、bash、Shell 调用 unity。",
            self.status.label(),
            self.project_path.display(),
            if project_ok {
                ""
            } else {
                "（非 Unity 工程根；聊天任务目录无效）"
            },
            self.pipeline_status.label(),
            self.pipeline_summary,
            self.editor_link.label(),
            self.loop_phase.label(),
            self.scene.status_line(),
            self.scene.note,
            latest,
        )
    }

    /// Agent-only context for the "analyze in chat" action. This text is not
    /// rendered in the timeline; the user sees a short intent label instead.
    pub fn compact_chat_briefing(&self) -> String {
        let latest = self
            .history
            .first()
            .map(|record| format!("{} / {}", record.title, record.summary))
            .unwrap_or_else(|| "无".into());
        format!(
            "你正在分析 Bony Build 的 Unity 面板快照。只读分析，禁止调用终端、unity、pipeline、command 或 eval。\n\
             状态：CLI={}；项目={}；Pipeline={}（{}）；编辑器={}；阶段={}；场景={}；最近操作={}。\n\
             回答规则：不要逐项复述快照；先用一句话给出结论，再给最多 3 条可操作建议；\
             若状态正常，明确说“连接正常”，不要建议重启、重装或登录；总计不超过 120 个汉字。",
            self.status.label(),
            display_path(self.project_path.clone()).display(),
            self.pipeline_status.label(),
            self.pipeline_summary,
            self.editor_link.label(),
            self.loop_phase.label(),
            self.scene.status_line(),
            latest,
        )
    }

    pub fn set_project_path(&mut self, path: PathBuf) {
        let resolved = resolve_unity_project_root(&path).unwrap_or_else(|| path.clone());
        if self.project_path == resolved {
            return;
        }
        if resolved != path {
            self.toast = Some(format!(
                "已自动定位工程根：{}（原路径在子目录内）",
                resolved.display()
            ));
        }
        self.project_path = resolved.clone();
        self.project_locked = looks_like_unity_project(&resolved);
        if self.project_locked {
            save_unity_project_pref(&resolved);
        }
        if self.pipeline_status == PipelineStatus::Installed {
            self.pipeline_status = PipelineStatus::Unknown;
            self.pipeline_summary = "项目已切换，请刷新 Pipeline".into();
        }
        self.editor_link = EditorLinkStatus::Unknown;
        self.sync_setup_step();
    }

    /// Optionally adopt agent cwd only when it resolves to a Unity project and
    /// the user has not locked a dedicated Unity root.
    pub fn consider_agent_cwd(&mut self, cwd: &PathBuf) {
        if self.project_locked && looks_like_unity_project(&self.project_path) {
            return;
        }
        if let Some(root) = resolve_unity_project_root(cwd) {
            self.set_project_path(root);
            return;
        }
        // Agent worktree / non-Unity cwd: keep existing locked path, otherwise
        // surface the mismatch so the wizard asks the user to pick a project.
        if !looks_like_unity_project(&self.project_path) {
            self.project_path = cwd.clone();
            self.project_locked = false;
            self.sync_setup_step();
        }
    }

    /// Recompute the recommended onboarding step from live state.
    pub fn sync_setup_step(&mut self) {
        let next = if self.status != CliStatus::Ready {
            SetupStep::InstallCli
        } else if !looks_like_unity_project(&self.project_path) {
            SetupStep::PickProject
        } else if self.pipeline_status != PipelineStatus::Installed
            && self.editor_link != EditorLinkStatus::Connected
        {
            SetupStep::InstallPipeline
        } else if self.editor_link != EditorLinkStatus::Connected {
            SetupStep::ProbeEditor
        } else {
            SetupStep::RunLoop
        };
        self.setup_step = next;
    }

    pub fn focused_setup_step(&self) -> SetupStep {
        self.setup_focus.unwrap_or(self.setup_step)
    }

    pub fn step_state(&self, step: SetupStep) -> StepState {
        let cur = self.setup_step.index();
        let idx = step.index();
        if idx < cur {
            StepState::Done
        } else if idx == cur {
            StepState::Current
        } else {
            StepState::Locked
        }
    }

    pub fn run_setup_primary(&mut self) {
        match self.focused_setup_step() {
            SetupStep::InstallCli => {
                // Copy is handled in UI; here we re-detect after user installs.
                self.run_action(UnityAction::RefreshDetect);
            }
            SetupStep::DetectCli => self.run_action(UnityAction::RefreshDetect),
            SetupStep::PickProject => {
                if looks_like_unity_project(&self.project_path) {
                    self.toast = Some("Unity 工程已确认，进入下一步".into());
                    self.sync_setup_step();
                } else {
                    self.toast = Some(
                        "请点「选择 Unity 工程根目录」选含 Assets 的文件夹（不要用 task worktree）"
                            .into(),
                    );
                }
            }
            SetupStep::InstallPipeline => self.run_action(UnityAction::InstallPipeline),
            SetupStep::ProbeEditor => self.run_action(UnityAction::ProbeEditor),
            SetupStep::RunLoop => self.run_action(UnityAction::RunFullLoop),
        }
    }

    pub fn advance_after_cli_install_copied(&mut self) {
        self.toast = Some("安装命令已复制。在 PowerShell 执行后点「我已安装，重新检测」".into());
        self.setup_focus = Some(SetupStep::DetectCli);
    }

    pub fn pipeline_ready_for_commands(&self) -> bool {
        matches!(self.editor_link, EditorLinkStatus::Connected)
    }

    pub fn checklist(&self) -> Vec<(&'static str, bool, String)> {
        vec![
            (
                "Unity CLI",
                self.status == CliStatus::Ready,
                if self.status == CliStatus::Ready {
                    "已就绪".into()
                } else {
                    self.status.label().into()
                },
            ),
            (
                "项目路径",
                looks_like_unity_project(&self.project_path),
                if looks_like_unity_project(&self.project_path) {
                    self.project_path.display().to_string()
                } else {
                    format!(
                        "{}（非 Unity 工程根，请在引导里重新选择）",
                        self.project_path.display()
                    )
                },
            ),
            (
                "Pipeline 包",
                self.pipeline_status == PipelineStatus::Installed,
                format!(
                    "{} · {}",
                    self.pipeline_status.label(),
                    self.pipeline_summary
                ),
            ),
            (
                "编辑器响应",
                self.editor_link == EditorLinkStatus::Connected,
                format!("{} · {}", self.editor_link.label(), self.commands_summary),
            ),
        ]
    }

    pub fn run_action(&mut self, action: UnityAction) {
        if self.busy && !matches!(action, UnityAction::RunFullLoop) {
            return;
        }

        if matches!(action, UnityAction::RunFullLoop) {
            self.start_full_loop();
            return;
        }

        if matches!(action, UnityAction::RefreshDetect) {
            self.status = CliStatus::Unknown;
            self.busy = false;
            self.pending_rx = None;
            self.ensure_detecting();
            return;
        }

        if matches!(action, UnityAction::InstallPipeline) {
            self.pipeline_status = PipelineStatus::Installing;
            self.pipeline_summary = "正在执行 unity pipeline install…".into();
        }
        if matches!(action, UnityAction::ListPipeline) {
            self.pipeline_status = PipelineStatus::Checking;
        }
        if matches!(action, UnityAction::ListCommands | UnityAction::ProbeEditor) {
            self.editor_link = EditorLinkStatus::Checking;
        }

        if self.demo_mode || self.status != CliStatus::Ready {
            self.run_demo(action);
            return;
        }
        let Some(cli) = self.cli_path.clone() else {
            self.run_demo(action);
            return;
        };

        let project = self.project_path.clone();
        let (title, args, phase) = action.to_cli_args(&self.eval_input, &project);
        let command_display = format_command(&cli, &args);
        let timeout = if matches!(action, UnityAction::InstallPipeline) {
            INSTALL_TIMEOUT
        } else {
            CMD_TIMEOUT
        };
        self.busy = true;
        let (tx, rx) = mpsc::channel();
        self.pending_rx = Some(rx);
        thread::spawn(move || {
            let started = Instant::now();
            let result = run_unity_timeout(&cli, &args, timeout, Some(&project));
            let elapsed_ms = started.elapsed().as_millis() as u64;
            let ok = result.ok
                && (!matches!(action, UnityAction::Eval) || eval_output_succeeded(&result.stdout));
            let _ = tx.send(UnityWorkerMsg::CommandDone {
                action,
                title,
                command: command_display,
                phase,
                ok,
                stdout: result.stdout,
                stderr: result.stderr,
                elapsed_ms,
            });
        });
    }

    fn start_full_loop(&mut self) {
        if self.busy || !self.guide_queue.is_empty() {
            return;
        }
        self.demo_mode = self.status != CliStatus::Ready;
        self.scene = SceneSnapshot {
            player_y: 1.0,
            ground_collider_enabled: true,
            is_playing: false,
            last_eval_result: "—".into(),
            note: "准备复现：玩家有时从地板掉落".into(),
        };
        self.loop_phase = LoopPhase::Observe;
        self.guide_queue = vec![
            UnityAction::ObserveCollider,
            UnityAction::FixCollider,
            UnityAction::EnterPlayMode,
        ];
        self.guide_label = Some("闭环 0/3 · 准备中".into());
        self.guide_next_at = Some(Instant::now());
        self.toast = Some("开始完整闭环演示".into());
    }

    fn run_demo(&mut self, action: UnityAction) {
        let (title, phase, ok, summary, detail, command) = match action {
            UnityAction::ListEditors => (
                "列出已安装编辑器".into(),
                LoopPhase::Observe,
                true,
                "demo: 2 editors (6000.2.10f1, 6000.0.28f1)".into(),
                DEMO_EDITORS_JSON.into(),
                "unity editors --format json".into(),
            ),
            UnityAction::ListPipeline => (
                "刷新 Pipeline 列表".into(),
                LoopPhase::Observe,
                true,
                "demo: Pipeline: Installed".into(),
                format!(
                    "Project: {}\nPipeline: Installed (com.unity.pipeline)\n",
                    self.project_path.display()
                ),
                "unity pipeline list".into(),
            ),
            UnityAction::InstallPipeline => (
                "安装 com.unity.pipeline".into(),
                LoopPhase::Observe,
                true,
                "demo: Pipeline: Installed".into(),
                format!(
                    "Installing com.unity.pipeline into {}\nPipeline: Installed\nWait for Editor recompile, then run unity command.\n",
                    self.project_path.display()
                ),
                "unity pipeline install".into(),
            ),
            UnityAction::ListCommands => (
                "发现已注册命令".into(),
                LoopPhase::Observe,
                true,
                "demo: greet, eval, play, stop".into(),
                "greet — Log a greeting\neval — Evaluate C# in the Editor\nplay — Enter Play Mode\nstop — Exit Play Mode\n".into(),
                "unity command".into(),
            ),
            UnityAction::ProbeEditor => (
                "探测编辑器连接".into(),
                LoopPhase::Observe,
                true,
                "demo: editor connected · 4 commands".into(),
                "Connected to Editor\ngreet\neval\nplay\nstop\n".into(),
                "unity command".into(),
            ),
            UnityAction::Eval => {
                let expr = self.eval_input.clone();
                let fake = demo_eval_result(&expr, &self.scene);
                (
                    "Eval C# 表达式".into(),
                    LoopPhase::Act,
                    true,
                    format!("demo ← {fake}"),
                    format!("{{\n  \"ok\": true,\n  \"result\": {fake},\n  \"expr\": {expr:?}\n}}\n"),
                    format!("unity command eval {expr:?}"),
                )
            }
            UnityAction::ObserveCollider => (
                "观察：碰撞体已禁用".into(),
                LoopPhase::Observe,
                true,
                "demo: GroundCollider.enabled == false".into(),
                "Bug report: player sometimes falls through the floor.\nInspect: GameObject.Find(\"Ground\").GetComponent<Collider>().enabled → false\n".into(),
                "unity command eval \"return GameObject.Find(\\\"Ground\\\").GetComponent<Collider>().enabled;\"".into(),
            ),
            UnityAction::FixCollider => (
                "行动：重新启用碰撞体".into(),
                LoopPhase::Act,
                true,
                "demo: collider.enabled = true".into(),
                "Action: GroundCollider.enabled = true\nNo domain reload · eval returned true\n".into(),
                "unity command eval \"var c = GameObject.Find(\\\"Ground\\\").GetComponent<Collider>(); c.enabled = true; return c.enabled;\"".into(),
            ),
            UnityAction::EnterPlayMode => (
                "验证：进入 Play Mode".into(),
                LoopPhase::Verify,
                true,
                "demo: isPlaying = true · player stable".into(),
                "Enter Play Mode\nisPlaying → true\nPlayer remains on floor\n".into(),
                "unity command eval \"UnityEditor.EditorApplication.isPlaying = true; return UnityEditor.EditorApplication.isPlaying;\"".into(),
            ),
            UnityAction::ExitPlayMode => (
                "退出 Play Mode".into(),
                LoopPhase::Verify,
                true,
                "demo: isPlaying = false".into(),
                "{\n  \"isPlaying\": false\n}\n".into(),
                "unity command eval \"UnityEditor.EditorApplication.isPlaying = false; return UnityEditor.EditorApplication.isPlaying;\"".into(),
            ),
            UnityAction::RefreshDetect | UnityAction::RunFullLoop => return,
        };

        self.demo_mode = true;
        self.busy = true;
        let stdout = if matches!(action, UnityAction::ListEditors) {
            DEMO_EDITORS_JSON.to_string()
        } else {
            format!("{summary}\n{detail}")
        };
        let (tx, rx) = mpsc::channel();
        self.pending_rx = Some(rx);
        let action_copy = action;
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(180));
            let _ = tx.send(UnityWorkerMsg::CommandDone {
                action: action_copy,
                title,
                command,
                phase,
                ok,
                stdout,
                stderr: String::new(),
                elapsed_ms: 12,
            });
        });
    }

    pub fn install_hint() -> &'static str {
        if cfg!(windows) {
            Self::install_hint_windows()
        } else {
            Self::install_hint_unix()
        }
    }

    pub fn install_hint_windows() -> &'static str {
        "$env:UNITY_CLI_CHANNEL='beta'; irm https://public-cdn.cloud.unity3d.com/hub/prod/cli/install.ps1 | iex"
    }

    pub fn install_hint_unix() -> &'static str {
        "curl -fsSL https://public-cdn.cloud.unity3d.com/hub/prod/cli/install.sh | UNITY_CLI_CHANNEL=beta bash"
    }

    pub fn take_toast(&mut self) -> Option<String> {
        self.toast.take()
    }

    pub fn needs_repaint(&self) -> bool {
        self.busy || !self.guide_queue.is_empty() || self.guide_next_at.is_some()
    }

    pub fn is_guiding(&self) -> bool {
        !self.guide_queue.is_empty() || self.guide_label.is_some()
    }
}

impl UnityAction {
    fn to_cli_args(self, eval_input: &str, project: &PathBuf) -> (String, Vec<String>, LoopPhase) {
        // `Path::canonicalize` returns an extended-length `\\?\C:\...` path on
        // Windows. Unity Pipeline registers editor instances under the normal
        // DOS/UNC spelling, so passing the extended spelling makes the CLI
        // miss an otherwise running editor.
        let project_s = path_for_unity_cli(project);
        match self {
            Self::RefreshDetect | Self::RunFullLoop => {
                ("重新检测 CLI".into(), vec!["--help".into()], LoopPhase::Observe)
            }
            Self::ListEditors => (
                "列出已安装编辑器".into(),
                vec!["editors".into(), "--format".into(), "json".into()],
                LoopPhase::Observe,
            ),
            Self::ListPipeline => (
                "刷新 Pipeline 列表".into(),
                vec!["pipeline".into(), "list".into()],
                LoopPhase::Observe,
            ),
            Self::InstallPipeline => (
                "安装 com.unity.pipeline".into(),
                vec!["pipeline".into(), "install".into()],
                LoopPhase::Observe,
            ),
            Self::ListCommands | Self::ProbeEditor => {
                let title = if matches!(self, Self::ProbeEditor) {
                    "探测编辑器连接"
                } else {
                    "发现已注册命令"
                };
                (
                    title.into(),
                    vec![
                        "command".into(),
                        format!("--project-path={project_s}"),
                    ],
                    LoopPhase::Observe,
                )
            }
            Self::Eval => (
                "Eval C# 表达式".into(),
                vec![
                    "--format".into(),
                    "json".into(),
                    "command".into(),
                    format!("--project-path={project_s}"),
                    "eval".into(),
                    "--".into(),
                    "--code".into(),
                    eval_input.to_string(),
                ],
                LoopPhase::Act,
            ),
            Self::ObserveCollider => (
                "观察碰撞体".into(),
                vec![
                    "--format".into(),
                    "json".into(),
                    "command".into(),
                    format!("--project-path={project_s}"),
                    "eval".into(),
                    "--".into(),
                    "--code".into(),
                    "var go = GameObject.Find(\"Ground\"); var c = go != null ? go.GetComponent<Collider>() : null; return c != null && c.enabled;".into(),
                ],
                LoopPhase::Observe,
            ),
            Self::FixCollider => (
                "修复碰撞体".into(),
                vec![
                    "--format".into(),
                    "json".into(),
                    "command".into(),
                    format!("--project-path={project_s}"),
                    "eval".into(),
                    "--".into(),
                    "--code".into(),
                    "var go = GameObject.Find(\"Ground\"); var c = go != null ? go.GetComponent<Collider>() : null; if (c != null) c.enabled = true; return c != null && c.enabled;".into(),
                ],
                LoopPhase::Act,
            ),
            Self::EnterPlayMode => (
                "进入 Play Mode".into(),
                vec![
                    "--format".into(),
                    "json".into(),
                    "command".into(),
                    format!("--project-path={project_s}"),
                    "eval".into(),
                    "--".into(),
                    "--code".into(),
                    "UnityEditor.EditorApplication.isPlaying = true; return UnityEditor.EditorApplication.isPlaying;".into(),
                ],
                LoopPhase::Verify,
            ),
            Self::ExitPlayMode => (
                "退出 Play Mode".into(),
                vec![
                    "--format".into(),
                    "json".into(),
                    "command".into(),
                    format!("--project-path={project_s}"),
                    "eval".into(),
                    "--".into(),
                    "--code".into(),
                    "UnityEditor.EditorApplication.isPlaying = false; return UnityEditor.EditorApplication.isPlaying;".into(),
                ],
                LoopPhase::Verify,
            ),
        }
    }
}

fn path_for_unity_cli(path: &PathBuf) -> String {
    let raw = path.display().to_string();
    #[cfg(windows)]
    {
        if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{rest}");
        }
        if let Some(rest) = raw.strip_prefix(r"\\?\") {
            return rest.to_string();
        }
    }
    raw
}

fn display_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        return PathBuf::from(path_for_unity_cli(&path));
    }
    #[cfg(not(windows))]
    path
}

struct DetectResult {
    path: Option<PathBuf>,
    version: String,
    error: Option<String>,
}

struct RunResult {
    ok: bool,
    stdout: String,
    stderr: String,
}

fn detect_cli() -> DetectResult {
    let candidates = candidate_bins();
    for path in candidates {
        let result = run_unity_timeout(&path, &["--help".into()], Duration::from_secs(8), None);
        if result.ok
            || result.stdout.contains("Usage")
            || result.stdout.to_lowercase().contains("unity")
        {
            let version = first_nonempty_line(&result.stdout)
                .or_else(|| first_nonempty_line(&result.stderr))
                .unwrap_or_else(|| "unity CLI".into());
            return DetectResult {
                path: Some(path),
                version,
                error: None,
            };
        }
    }

    match Command::new("unity").arg("--help").output() {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            if out.status.success()
                || stdout.contains("Usage")
                || stdout.to_lowercase().contains("unity")
            {
                let version = first_nonempty_line(&stdout)
                    .or_else(|| first_nonempty_line(&stderr))
                    .unwrap_or_else(|| "unity (PATH)".into());
                return DetectResult {
                    path: which_unity(),
                    version,
                    error: None,
                };
            }
            DetectResult {
                path: None,
                version: String::new(),
                error: Some(truncate_one_line(&stderr, 200)),
            }
        }
        Err(err) => DetectResult {
            path: None,
            version: String::new(),
            error: Some(format!("无法启动 unity: {err}")),
        },
    }
}

fn candidate_bins() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(p) = std::env::var("UNITY_CLI") {
        out.push(PathBuf::from(p));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let base = PathBuf::from(local);
        out.push(base.join("Unity").join("bin").join("unity.exe"));
        out.push(base.join("Unity").join("cli").join("unity.exe"));
        out.push(
            base.join("Programs")
                .join("Unity")
                .join("cli")
                .join("unity.exe"),
        );
        out.push(
            base.join("Programs")
                .join("Unity")
                .join("bin")
                .join("unity.exe"),
        );
    }
    if let Some(home) = std::env::var_os("USERPROFILE") {
        let base = PathBuf::from(home);
        out.push(base.join(".unity").join("bin").join("unity.exe"));
        out.push(base.join(".unity").join("cli").join("unity.exe"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let base = PathBuf::from(home);
        out.push(base.join(".unity").join("bin").join("unity"));
        out.push(base.join(".local").join("bin").join("unity"));
    }
    out.into_iter().filter(|p| p.exists()).collect()
}

fn which_unity() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let output = Command::new("where.exe").arg("unity").output().ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        text.lines().next().map(|l| PathBuf::from(l.trim()))
    }
    #[cfg(not(windows))]
    {
        let output = Command::new("sh")
            .args(["-c", "command -v unity"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let line = text.lines().next()?.trim();
        if line.is_empty() {
            None
        } else {
            Some(PathBuf::from(line))
        }
    }
}

fn run_unity_timeout(
    bin: &PathBuf,
    args: &[String],
    timeout: Duration,
    cwd: Option<&PathBuf>,
) -> RunResult {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("CI", "1")
        .env("UNITY_CLI_NONINTERACTIVE", "1");
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(err) => {
            return RunResult {
                ok: false,
                stdout: String::new(),
                stderr: format!("spawn failed: {err}"),
            };
        }
    };

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return RunResult {
                    ok: false,
                    stdout: String::new(),
                    stderr: format!("timeout after {}s", timeout.as_secs()),
                };
            }
            Ok(None) => thread::sleep(Duration::from_millis(40)),
            Err(err) => {
                return RunResult {
                    ok: false,
                    stdout: String::new(),
                    stderr: format!("wait failed: {err}"),
                };
            }
        }
    };

    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut stdout);
    }
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut stderr);
    }
    RunResult {
        ok: status.success(),
        stdout,
        stderr,
    }
}

fn format_command(bin: &PathBuf, args: &[String]) -> String {
    let mut parts = vec![bin.display().to_string()];
    for a in args {
        if a.contains(' ') {
            parts.push(format!("\"{a}\""));
        } else {
            parts.push(a.clone());
        }
    }
    parts.join(" ")
}

fn summarize_editors_json(raw: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return truncate_one_line(raw, 120);
    };
    if let Some(arr) = value.as_array() {
        if arr.is_empty() {
            return "0 个已安装编辑器".into();
        }
        let versions: Vec<String> = arr
            .iter()
            .filter_map(|v| {
                v.get("version")
                    .or_else(|| v.get("Version"))
                    .and_then(|x| x.as_str())
                    .map(str::to_string)
            })
            .take(4)
            .collect();
        if versions.is_empty() {
            return format!("{} 个编辑器", arr.len());
        }
        return format!("{} 个：{}", arr.len(), versions.join(", "));
    }
    truncate_one_line(raw, 120)
}

fn summarize_pipeline_list(raw: &str) -> String {
    let lower = raw.to_lowercase();
    // The CLI table may wrap between any header columns. Detect the two
    // relevant labels independently so `Server\nReachable` is still valid.
    if lower.contains("server") && lower.contains("reachable") {
        let version = raw
            .split_whitespace()
            .find(|field| {
                field.chars().next().is_some_and(|c| c.is_ascii_digit()) && field.contains('.')
            })
            .unwrap_or("未知版本");
        return match pipeline_server_reachable(raw) {
            Some(true) => format!("Pipeline {version} · 编辑器服务可达"),
            Some(false) => format!("Pipeline {version} · 包已登记，编辑器服务尚未启动"),
            None => format!("Pipeline {version} · 尚未识别编辑器服务状态"),
        };
    }
    if lower.contains("server reachable") {
        let data = raw
            .lines()
            .filter(|line| !line.trim().is_empty())
            .skip(1)
            .find(|line| line.split_whitespace().count() > 2);
        if let Some(line) = data {
            let fields: Vec<_> = line.split_whitespace().collect();
            let version = fields
                .iter()
                .find(|field| {
                    field.chars().next().is_some_and(|c| c.is_ascii_digit()) && field.contains('.')
                })
                .copied()
                .unwrap_or("未知版本");
            let reachable = pipeline_server_reachable(raw).unwrap_or(false);
            return if reachable {
                format!("Pipeline {version} · 编辑器服务可达")
            } else {
                format!("Pipeline {version} · 包已登记，编辑器服务尚未启动")
            };
        }
        "Pipeline 已登记 · 尚未发现编辑器服务".into()
    } else if lower.contains("installed") {
        "Pipeline: Installed".into()
    } else if raw.lines().filter(|l| !l.trim().is_empty()).count() == 0 {
        "无 Pipeline 项目".into()
    } else {
        truncate_one_line(raw, 160)
    }
}

/// Finds the reachability boolean using the server port as a stable anchor.
/// This survives missing PID values and terminal line wrapping.
fn pipeline_server_reachable(raw: &str) -> Option<bool> {
    let fields: Vec<_> = raw.split_whitespace().collect();
    let version_index = fields.iter().position(|field| {
        field.chars().next().is_some_and(|c| c.is_ascii_digit()) && field.contains('.')
    })?;
    fields[version_index + 1..].windows(2).find_map(|pair| {
        pair[0].parse::<u16>().ok()?;
        match pair[1].to_ascii_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }
    })
}

fn infer_pipeline_status(raw: &str, ok: bool) -> PipelineStatus {
    if !ok {
        return PipelineStatus::Error;
    }
    let lower = raw.to_lowercase();
    if lower.contains("not installed") || lower.contains("notinstalled") {
        PipelineStatus::NotInstalled
    } else if lower.contains("installed")
        || lower.contains("com.unity.pipeline")
        || lower.contains("pipeline: installed")
    {
        PipelineStatus::Installed
    } else if raw.trim().is_empty() {
        PipelineStatus::NotInstalled
    } else {
        // Non-empty list output usually means at least one project.
        PipelineStatus::Installed
    }
}

fn summarize_eval_output(raw: &str) -> String {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Some(result) = value
            .pointer("/data/result")
            .or_else(|| value.get("result"))
        {
            return truncate_one_line(&result.to_string(), 160);
        }
        if value.get("success").and_then(|v| v.as_bool()) == Some(false) {
            return value
                .pointer("/errors/0/message")
                .and_then(|v| v.as_str())
                .map(|v| truncate_one_line(v, 160))
                .unwrap_or_else(|| "Unity Eval 失败".into());
        }
    }
    raw.lines()
        .rev()
        .map(str::trim)
        .find(|line| {
            !line.is_empty()
                && !line
                    .to_ascii_lowercase()
                    .contains("command success result parameters")
        })
        .map(|line| truncate_one_line(line, 160))
        .unwrap_or_else(|| "Unity Eval 未返回结果".into())
}

fn eval_output_succeeded(raw: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return false;
    };
    if value.get("success").and_then(|v| v.as_bool()) == Some(false) {
        return false;
    }
    if value.pointer("/data/success").and_then(|v| v.as_bool()) == Some(false) {
        return false;
    }
    value.get("success").and_then(|v| v.as_bool()) == Some(true)
        || value.pointer("/data/success").and_then(|v| v.as_bool()) == Some(true)
}

fn infer_editor_link(raw: &str, ok: bool) -> EditorLinkStatus {
    if !ok {
        return EditorLinkStatus::Disconnected;
    }
    let lower = raw.to_lowercase();
    if lower.contains("no editor")
        || lower.contains("not connected")
        || lower.contains("could not")
        || lower.contains("failed")
    {
        EditorLinkStatus::Disconnected
    } else if raw.trim().is_empty() {
        EditorLinkStatus::Disconnected
    } else {
        EditorLinkStatus::Connected
    }
}

/// Public helpers for the desktop UI.
pub fn is_unity_project_root(path: &PathBuf) -> bool {
    looks_like_unity_project(path)
}

pub fn find_unity_project_root(path: &PathBuf) -> Option<PathBuf> {
    resolve_unity_project_root(path)
}

fn looks_like_unity_project(path: &PathBuf) -> bool {
    path.join("Assets").is_dir()
        && (path.join("ProjectSettings").is_dir()
            || path.join("Packages").join("manifest.json").is_file())
}

fn pipeline_declared(project: &PathBuf) -> bool {
    std::fs::read_to_string(project.join("Packages").join("manifest.json"))
        .is_ok_and(|text| text.contains("\"com.unity.pipeline\""))
}

fn pipeline_loaded_by_editor(project: &PathBuf) -> bool {
    let locked = std::fs::read_to_string(project.join("Packages").join("packages-lock.json"))
        .is_ok_and(|text| text.contains("\"com.unity.pipeline\""));
    let cached = std::fs::read_dir(project.join("Library").join("PackageCache"))
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("com.unity.pipeline@")
        });
    locked && cached
}

/// Walk up from `path` until we find a Unity project root (Assets + ProjectSettings/Packages).
/// Handles cases where cwd is inside `Assets/...` or another subfolder.
fn resolve_unity_project_root(path: &PathBuf) -> Option<PathBuf> {
    let mut cur = path.canonicalize().unwrap_or_else(|_| path.clone());
    for _ in 0..12 {
        if looks_like_unity_project(&cur) {
            return Some(display_path(cur));
        }
        if !cur.pop() {
            break;
        }
    }
    None
}

fn unity_project_pref_path() -> PathBuf {
    crate::usage::usage_dir().join("unity_project.json")
}

fn load_unity_project_pref() -> Option<PathBuf> {
    let text = std::fs::read_to_string(unity_project_pref_path()).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let path = value.get("path")?.as_str()?;
    let p = PathBuf::from(path);
    p.is_dir().then_some(p)
}

fn save_unity_project_pref(path: &PathBuf) {
    let dir = crate::usage::usage_dir();
    let _ = std::fs::create_dir_all(&dir);
    let body = serde_json::json!({ "path": path });
    if let Ok(text) = serde_json::to_string_pretty(&body) {
        let _ = std::fs::write(unity_project_pref_path(), text);
    }
}

fn demo_eval_result(expr: &str, scene: &SceneSnapshot) -> String {
    let lower = expr.to_lowercase();
    if lower.contains("isplaying") {
        return if scene.is_playing {
            "true".into()
        } else {
            "false".into()
        };
    }
    if lower.contains("collider") || lower.contains("enabled") {
        return if scene.ground_collider_enabled {
            "true".into()
        } else {
            "false".into()
        };
    }
    if lower.contains("version") {
        return "\"6000.2.10f1\"".into();
    }
    if lower.contains("datapath") {
        return "\"C:/Projects/DemoGame/Assets\"".into();
    }
    "null".into()
}

fn merge_streams(stdout: &str, stderr: &str) -> String {
    if stderr.is_empty() {
        stdout.to_string()
    } else if stdout.is_empty() {
        stderr.to_string()
    } else {
        format!("{stdout}\n--- stderr ---\n{stderr}")
    }
}

fn truncate_one_line(s: &str, max: usize) -> String {
    let line = s.lines().find(|l| !l.trim().is_empty()).unwrap_or(s).trim();
    if line.chars().count() <= max {
        line.to_string()
    } else {
        let mut out: String = line.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn first_nonempty_line(s: &str) -> Option<String> {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

pub fn format_relative(at_unix: u64) -> String {
    let now = now_unix();
    let secs = now.saturating_sub(at_unix);
    if secs < 5 {
        "刚刚".into()
    } else if secs < 60 {
        format!("{secs} 秒前")
    } else if secs < 3600 {
        format!("{} 分钟前", secs / 60)
    } else if secs < 86400 {
        format!("{} 小时前", secs / 3600)
    } else {
        format!("{} 天前", secs / 86400)
    }
}

const DEMO_EDITORS_JSON: &str = r#"[
  {"version":"6000.2.10f1","modules":["android","ios","webgl"]},
  {"version":"6000.0.28f1","modules":["android"]}
]"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_demo_editors() {
        let s = summarize_editors_json(DEMO_EDITORS_JSON);
        assert!(s.contains("6000.2.10f1"));
        assert!(s.contains("2"));
    }

    #[test]
    fn full_loop_queues_three_steps() {
        let mut state = UnityState::default();
        state.status = CliStatus::Missing;
        state.demo_mode = true;
        state.run_action(UnityAction::RunFullLoop);
        assert_eq!(state.guide_queue.len(), 3);
        assert!(state.guide_label.is_some());
    }

    #[test]
    fn resolve_root_from_assets_subdir() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        std::fs::create_dir_all(root.join("Assets").join("Skyboxes")).unwrap();
        std::fs::create_dir_all(root.join("ProjectSettings")).unwrap();
        let nested = root.join("Assets").join("Skyboxes");
        let resolved = resolve_unity_project_root(&nested).unwrap();
        assert_eq!(
            resolved.canonicalize().unwrap(),
            root.canonicalize().unwrap()
        );
    }

    #[test]
    fn unity_cli_path_drops_windows_extended_prefix() {
        let path = PathBuf::from(r"\\?\C:\Users\测试\UnityProject");
        #[cfg(windows)]
        assert_eq!(path_for_unity_cli(&path), r"C:\Users\测试\UnityProject");
        #[cfg(not(windows))]
        assert_eq!(path_for_unity_cli(&path), path.display().to_string());
    }

    #[test]
    fn pipeline_summary_distinguishes_package_from_live_server() {
        let raw = "Project Path PID Running Pipeline Version Update Available Server Port Server Reachable Safe Mode\n教程 C:\\Unity\\教程 123 true true 0.3.1-exp.1 false 0 false false";
        let summary = summarize_pipeline_list(raw);
        assert!(summary.contains("0.3.1-exp.1"));
        assert!(summary.contains("尚未启动"));
    }

    #[test]
    fn detects_manifest_only_pipeline_install() {
        let tmp = tempfile::tempdir().unwrap();
        let packages = tmp.path().join("Packages");
        std::fs::create_dir_all(&packages).unwrap();
        std::fs::write(
            packages.join("manifest.json"),
            r#"{"dependencies":{"com.unity.pipeline":"0.3.1-exp.1"}}"#,
        )
        .unwrap();
        assert!(pipeline_declared(&tmp.path().to_path_buf()));
        assert!(!pipeline_loaded_by_editor(&tmp.path().to_path_buf()));
    }

    #[test]
    fn detects_reachable_pipeline_with_wrapped_header_and_missing_pid() {
        let raw = "Project Path PID Running Pipeline Version Update Available Server Port Server\nReachable Safe Mode\nTutorial C:\\Unity\\Tutorial true true 0.3.1-exp.1 false 7800 true";
        assert_eq!(pipeline_server_reachable(raw), Some(true));
        let summary = summarize_pipeline_list(raw);
        assert!(summary.contains("0.3.1-exp.1"));
    }

    #[test]
    fn detects_unreachable_pipeline_using_port_anchor() {
        let raw = "Project Path PID Running Pipeline Version Update Available Server Port Server Reachable Safe Mode\nTutorial C:\\Unity\\Tutorial 123 true true 0.3.1-exp.1 false 7800 false false";
        assert_eq!(pipeline_server_reachable(raw), Some(false));
    }

    #[test]
    fn routes_scene_sphere_request_to_unity_eval() {
        let cmd = parse_unity_chat_command("帮我在场景画一个球体").unwrap();
        assert_eq!(cmd.action, UnityAction::Eval);
        assert!(cmd.eval.unwrap().contains("CreatePrimitive"));
        assert_eq!(cmd.slash, "/unity sphere");
    }

    #[test]
    fn compiles_parameterized_scene_creation() {
        let (label, eval) = compile_unity_scene_command("帮我创建3个球体并排放置").unwrap();
        assert_eq!(label, "创建 3 个球体");
        assert!(eval.contains("i < 3"));
        assert!(eval.contains("PrimitiveType.Sphere"));

        let (_, cube_eval) = compile_unity_scene_command("生成五个立方体").unwrap();
        assert!(cube_eval.contains("i < 5"));
        assert!(cube_eval.contains("PrimitiveType.Cube"));
    }

    #[test]
    fn compiles_follow_up_color_edit_for_selection() {
        let (label, eval) = compile_unity_scene_command("帮我补上绿色").unwrap();
        assert_eq!(label, "把选中对象设为绿色");
        assert!(eval.contains("UnityEditor.Selection.gameObjects"));
        assert!(eval.contains("Color.green"));
        assert!(eval.contains("GetComponentsInChildren<Renderer>"));
    }

    #[test]
    fn compiles_common_selection_edits() {
        assert!(
            compile_unity_scene_command("把它们向上移动")
                .unwrap()
                .1
                .contains("Vector3.up")
        );
        assert!(
            compile_unity_scene_command("把这些放大")
                .unwrap()
                .1
                .contains("2f")
        );
        assert!(
            compile_unity_scene_command("给它们添加刚体")
                .unwrap()
                .1
                .contains("Rigidbody")
        );
        assert!(
            compile_unity_scene_command("复制选中对象")
                .unwrap()
                .1
                .contains("Instantiate")
        );
        assert!(
            compile_unity_scene_command("删除选中对象")
                .unwrap()
                .1
                .contains("DestroyObjectImmediate")
        );
    }

    #[test]
    fn accepts_safe_generated_unity_plan() {
        let raw = r#"{"summary":"创建灯光","csharp":"var go = new GameObject(\"Light\"); UnityEditor.Undo.RegisterCreatedObjectUndo(go, \"Create Light\"); go.AddComponent<Light>(); return go.name;"}"#;
        let (summary, csharp) = parse_generated_unity_plan(raw).unwrap();
        assert_eq!(summary, "创建灯光");
        assert!(csharp.contains("AddComponent<Light>"));
    }

    #[test]
    fn rejects_generated_plan_with_external_io() {
        let raw = r#"{"summary":"write","csharp":"System.IO.File.WriteAllText(\"x\", \"y\"); return true;"}"#;
        assert!(parse_generated_unity_plan(raw).is_err());
    }

    #[test]
    fn eval_uses_named_code_argument_and_json_output() {
        let (_, args, _) =
            UnityAction::Eval.to_cli_args("return 42;", &PathBuf::from(r"C:\Unity\Project"));
        assert_eq!(&args[0..2], &["--format", "json"]);
        assert!(args.windows(2).any(|pair| pair == ["--code", "return 42;"]));
        assert!(args.iter().any(|arg| arg == "--"));
    }

    #[test]
    fn eval_requires_structured_success() {
        assert!(eval_output_succeeded(
            r#"{"success":true,"data":{"success":true,"result":"Created 30 Cube"}}"#
        ));
        assert!(!eval_output_succeeded(
            r#"{"success":true,"data":{"success":false,"result":"compile error"}}"#
        ));
        assert!(!eval_output_succeeded("Command Success Result Parameters"));
    }
}
