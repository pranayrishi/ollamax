use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "forge",
    author = "Ollama-Forge Team",
    version,
    about = "⚒️ The 'Everything' Framework for Local Coding Agents",
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

    #[command(about = "Build a feature or application")]
    Build {
        #[arg(help = "The task or feature to build")]
        task: Vec<String>,

        #[arg(short, long, help = "Output directory")]
        output: Option<PathBuf>,

        #[arg(
            short,
            long,
            help = "Language/framework (auto-detect if not specified)"
        )]
        lang: Option<String>,

        #[arg(short, long, help = "Run tests after building")]
        test: bool,

        #[arg(short, long, help = "Skip security checks")]
        no_security: bool,
    },

    #[command(about = "Chat with a local model")]
    Chat {
        #[arg(short, long, help = "Model to use")]
        model: Option<String>,

        #[arg(help = "Initial prompt")]
        prompt: Option<String>,
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

    #[command(about = "Manage skills and recipes")]
    Skills {
        #[command(subcommand)]
        action: SkillsAction,
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
}

#[derive(ValueEnum, Debug, Clone)]
pub enum AnalysisType {
    Complexity,
    Security,
    Performance,
    Style,
    Full,
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

impl Cli {
    pub fn build_request(&self) -> Option<crate::orchestrator::BuildRequest> {
        match &self.command {
            Commands::Build {
                task,
                output,
                lang,
                test,
                no_security,
            } => Some(crate::orchestrator::BuildRequest {
                task: task.join(" "),
                output_dir: output.clone(),
                language: lang.clone(),
                run_tests: *test,
                skip_security: *no_security,
            }),
            _ => None,
        }
    }
}
