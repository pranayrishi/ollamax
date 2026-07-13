use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Version string baked at compile time. Includes the git short SHA so
/// `forge --version` and the future deterministic-replay log can pin a
/// specific build. The SHA comes from `build.rs`.
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (", env!("FORGE_GIT_SHA"), ")");

#[derive(Parser, Debug)]
#[command(
    name = "forge",
    author = "Pranay Rishi Nalem",
    version = VERSION,
    about = "Harness optimization layer for local coding agents running on Ollama",
    long_about = None,
    infer_subcommands = true,
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    #[arg(short, long, global = true, help = "Verbose output")]
    pub verbose: bool,

    #[arg(short, long, global = true, help = "Quiet mode (less output)")]
    pub quiet: bool,

    #[arg(short, long, global = true, help = "Config file path")]
    pub config: Option<PathBuf>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Commands {
    #[command(about = "Initialize Forge in the current directory")]
    Init {
        #[arg(short, long, help = "Force re-initialization")]
        force: bool,
    },

    #[command(about = "Build a feature or application via the parallel orchestrator")]
    Build {
        #[arg(help = "The task or feature to build")]
        task: Vec<String>,

        #[arg(short, long, help = "Skip the post-build security scan")]
        no_security: bool,

        #[arg(
            short = 'o',
            long,
            help = "Write extracted code blocks (\"```LANG path\\n...```\") into this directory"
        )]
        output: Option<PathBuf>,
    },

    #[command(about = "Chat with a local model")]
    Chat {
        #[arg(short, long, help = "Model to use")]
        model: Option<String>,

        #[arg(help = "Initial prompt")]
        prompt: Option<String>,
    },

    #[command(about = "Run a local workspace agent that can inspect, edit, and validate files")]
    Agent {
        #[arg(
            short,
            long,
            help = "Model to use (defaults to the best installed local model)"
        )]
        model: Option<String>,

        #[arg(
            long,
            default_value = "12",
            help = "Maximum bounded tool rounds (inventory, read, edit, validate)"
        )]
        max_iterations: usize,

        #[arg(
            long,
            value_enum,
            default_value_t = AgentAutonomy::Confirm,
            help = "Whether consequential file writes and shell commands need approval"
        )]
        autonomy: AgentAutonomy,

        #[arg(
            short = 'y',
            long,
            help = "Approve consequential actions without prompting (equivalent to --autonomy auto)"
        )]
        yes: bool,

        #[arg(help = "Task for the agent to perform in the current directory")]
        task: Vec<String>,
    },

    #[command(
        about = "Coordinate read-only local scouts, one workspace writer, deterministic verification, and an advisory review"
    )]
    Team {
        #[arg(
            short,
            long,
            help = "Model used by scouts and the controlled writer (defaults to the best installed local model)"
        )]
        model: Option<String>,

        #[arg(
            long,
            help = "Optional smaller installed local model for read-only scouts. When omitted, Ollamax uses an installed configured execution model only when it is no larger than the writer; otherwise scouts use --model."
        )]
        scout_model: Option<String>,

        #[arg(
            long,
            help = "Optional installed local model for the read-only scout synthesis/planning hand-off. Defaults to the configured planning model when installed, otherwise --model."
        )]
        planner_model: Option<String>,

        #[arg(
            long,
            help = "Optional local model used only for the final read-only review (defaults to --model)"
        )]
        reviewer_model: Option<String>,

        #[arg(
            long,
            default_value = "12",
            help = "Maximum bounded tool rounds for each controlled writer pass"
        )]
        max_iterations: usize,

        #[arg(
            long,
            default_value = "1",
            help = "Maximum repair passes after failed deterministic verification"
        )]
        max_repair_rounds: usize,

        #[arg(
            long,
            help = "Run the two read-only scouts concurrently; this uses more RAM/VRAM but never enables concurrent writers"
        )]
        parallel_scouts: bool,

        #[arg(
            long,
            value_enum,
            default_value_t = AgentAutonomy::Confirm,
            help = "Whether workspace writes and fixed verification commands need approval"
        )]
        autonomy: AgentAutonomy,

        #[arg(
            short = 'y',
            long,
            help = "Approve controlled writer actions and fixed verification commands without prompting"
        )]
        yes: bool,

        #[arg(help = "Task for the local coding team to perform in the current directory")]
        task: Vec<String>,
    },

    #[command(about = "Analyze code and suggest improvements")]
    Analyze {
        #[arg(help = "File or directory to analyze")]
        path: PathBuf,

        #[arg(short, long, help = "Analysis type")]
        analysis_type: Option<AnalysisType>,
    },

    #[command(about = "Run security audit on code")]
    Audit {
        #[arg(help = "Directory to audit")]
        path: PathBuf,

        #[arg(short, long, help = "Include secrets detection")]
        secrets: bool,

        #[arg(
            long,
            help = "Emit findings as JSON to stdout (for CI / pre-commit consumers)"
        )]
        json: bool,
    },

    #[command(about = "Execute tasks in parallel")]
    Parallel {
        #[arg(help = "Number of workers")]
        workers: Option<usize>,

        #[arg(help = "Tasks to execute")]
        tasks: Vec<String>,
    },

    #[command(about = "Generate tests for code")]
    Test {
        #[arg(help = "File or directory to generate tests for")]
        path: PathBuf,

        #[arg(short, long, help = "Test framework")]
        framework: Option<String>,
    },

    #[command(about = "Show system information and hardware stats")]
    Status {
        #[arg(short, long, help = "Show detailed model information")]
        models: bool,
    },

    #[command(
        about = "List curated free open-weight models, tiered to your hardware \
                       (with license + what's installed + the recommended default)"
    )]
    Models {
        #[arg(
            long,
            help = "Verify each tag against the live Ollama library (networked, slower)"
        )]
        verify: bool,
        #[arg(long, help = "Only show models your detected VRAM can actually run")]
        fits_only: bool,
    },

    #[command(about = "Manage skills and recipes")]
    Skills {
        #[command(subcommand)]
        action: SkillsAction,
    },

    #[command(
        about = "Manage curated GitHub knowledge plugins (documentation-only; repositories are never executed or installed as code)"
    )]
    Plugins {
        #[command(subcommand)]
        action: PluginsAction,
    },

    #[command(
        about = "Validate local evaluation scenarios and score append-only local benchmark results"
    )]
    Eval {
        #[command(subcommand)]
        action: EvalAction,
    },

    #[command(
        name = "run-skill",
        about = "Run a specific skill against a task (uses the skill's system prompt + recommended model)"
    )]
    RunSkill {
        #[arg(help = "Skill name (e.g. docker-expert) — see `forge skills list`")]
        name: String,

        #[arg(help = "Task description, e.g. 'containerize a node app'")]
        task: Vec<String>,
    },

    #[command(about = "Run the research agent against a question. \
                 Uses local Ollama for inference + free public tools \
                 (DuckDuckGo, Wikipedia, arXiv, plain HTTP) for sources.")]
    Research {
        #[arg(help = "Question to research")]
        question: Vec<String>,

        #[arg(
            short,
            long,
            help = "Override the model used by the agent (default: config default_model)"
        )]
        model: Option<String>,

        #[arg(long, help = "Print the full tool-call trace to stderr")]
        trace: bool,

        #[arg(
            long,
            default_value = "6",
            help = "Maximum number of tool-call rounds before forcing an answer"
        )]
        max_iterations: usize,
    },

    #[command(about = "List the tools the research agent can call")]
    Tools,

    #[command(
        about = "Bootstrap a local LoRA fine-tune of a Qwen2.5-Coder model on your codebase. \
                 Runs the `lora-finetune` skill against your repo. Output is a set of scripts \
                 you run yourself — no model training happens inside forge. Local-only, no \
                 hosted services."
    )]
    Finetune {
        #[arg(
            help = "Path to the repo to fine-tune on (defaults to current directory)",
            default_value = "."
        )]
        repo: PathBuf,

        #[arg(
            short,
            long,
            help = "Override the model used by the planner (default: largest installed)"
        )]
        model: Option<String>,
    },

    #[command(
        about = "Manage user 'always-rules' that get prepended to every system prompt. \
                 Drop Markdown files into ~/.config/ollama-forge/rules/ — they're picked \
                 up automatically by chat, research, run-skill, analyze, test, and build."
    )]
    Rules {
        #[command(subcommand)]
        action: RulesAction,
    },

    #[command(
        about = "Replay a deterministic log against the locally-installed model. \
                 Reports any response drift since the log was captured."
    )]
    Replay {
        #[arg(help = "Path to a JSON Lines replay log produced by an earlier forge run")]
        log: PathBuf,

        #[arg(long, help = "Print full responses (default: just hash drift)")]
        verbose: bool,
    },

    #[command(
        about = "Surface patterns from your replay log as candidate skills/rules. \
                 Read-only — does not auto-promote anything."
    )]
    Instincts {
        #[arg(help = "Path to a JSON Lines replay log (default: $FORGE_REPLAY_LOG)")]
        log: Option<PathBuf>,

        #[arg(
            short,
            long,
            default_value = "3",
            help = "Minimum number of times a pattern must repeat before it surfaces"
        )]
        threshold: usize,
    },

    #[command(about = "Warm-load a model into Ollama (avoids cold-start on the next call)")]
    Preload {
        #[arg(
            help = "Model name (e.g. qwen2.5-coder:7b). Defaults to your config's `default_model`."
        )]
        model: Option<String>,

        #[arg(
            short,
            long,
            default_value = "1h",
            help = "How long Ollama should keep the model resident. Accepts `30m`, `1h`, `0` (immediate unload)."
        )]
        keep_alive: String,
    },

    #[command(about = "Optimize Ollama settings for your hardware")]
    Optimize {
        #[arg(short, long, help = "Aggressive optimization")]
        aggressive: bool,

        #[arg(short, long, help = "Show changes without applying")]
        dry_run: bool,
    },

    #[command(
        about = "Run the local backend server for the desktop app / VSCode extension. \
                 Binds 127.0.0.1 only (never 0.0.0.0). Exposes chat/research/build over \
                 a streaming (SSE) API. The CLI is unaffected; this is additive."
    )]
    Serve {
        #[arg(
            short,
            long,
            default_value = "7878",
            help = "Port to bind on 127.0.0.1. Use 0 for an OS-assigned port (printed on startup)."
        )]
        port: u16,

        #[arg(
            long,
            default_value = "127.0.0.1",
            help = "Host to bind. Forced to loopback; 0.0.0.0 and external hosts are refused."
        )]
        host: String,
    },
}

#[derive(ValueEnum, Debug, Clone)]
pub enum AnalysisType {
    Complexity,
    Security,
    Performance,
    Style,
    Full,
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentAutonomy {
    /// Show the proposed plan and ask before every file write or shell command.
    Confirm,
    /// Run filesystem edits within the current workspace without per-step
    /// prompts. Host-shell actions, including repository-defined test scripts,
    /// run from that directory but are not OS-sandboxed.
    Auto,
    /// Inspect/search only; deny writes and shell commands.
    Readonly,
}

#[derive(Subcommand, Debug, Clone)]
pub enum RulesAction {
    /// List installed rules.
    List,
    /// Print the absolute path to the rules directory.
    Path,
    /// Create the rules directory and a starter rule file.
    Init,
    /// Print the rendered concatenation that gets injected into prompts.
    Show,
    /// Open `$EDITOR` (or `$VISUAL`) on the rules directory.
    Edit {
        /// Open this specific rule file instead of the directory.
        /// `forge rules edit 00-style` opens `00-style.md`.
        #[arg(help = "Optional rule name (without .md). Opens the dir when omitted.")]
        name: Option<String>,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum SkillsAction {
    List,
    Add {
        #[arg(help = "Skill file or URL")]
        source: String,
    },
    Remove {
        #[arg(help = "Skill name")]
        name: String,
    },
    Search {
        #[arg(help = "Search query")]
        query: String,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum PluginsAction {
    /// Show the embedded curated catalog and whether each entry is installed.
    List,
    /// Fetch a curated repository README with provenance and policy checks.
    Install {
        #[arg(help = "Curated plugin id (see `forge plugins list`)")]
        id: String,
    },
    /// Remove one locally cached knowledge plugin; no remote request is made.
    Remove {
        #[arg(help = "Installed plugin id")]
        id: String,
    },
    /// Show which installed knowledge-plugin documents match a task, without running a model.
    Context {
        #[arg(help = "Task or query used for relevance matching")]
        query: Vec<String>,

        #[arg(long, default_value = "3", help = "Maximum matching plugins to show (1-5)")]
        max_plugins: usize,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum EvalAction {
    /// Validate a JSON or TOML scenario without running a model or command.
    Validate {
        #[arg(help = "Scenario .json or .toml file")]
        scenario: PathBuf,
    },
    /// Score an append-only local JSONL evaluation log.
    Report {
        #[arg(help = "JSONL result log")]
        results: PathBuf,

        #[arg(long, help = "Emit the score report as JSON")]
        json: bool,
    },
    /// Compare candidate local results against a baseline JSONL log.
    Compare {
        #[arg(help = "Baseline JSONL result log")]
        baseline: PathBuf,

        #[arg(help = "Candidate JSONL result log")]
        candidate: PathBuf,

        #[arg(long, help = "Emit the comparison as JSON")]
        json: bool,
    },
}

impl Cli {
    pub fn build_request(&self) -> Option<crate::orchestrator::BuildRequest> {
        match &self.command {
            Commands::Build {
                task,
                no_security,
                output,
            } => Some(crate::orchestrator::BuildRequest {
                task: task.join(" "),
                output_dir: output.clone(),
                language: None,
                run_tests: false,
                skip_security: *no_security,
            }),
            _ => None,
        }
    }
}
