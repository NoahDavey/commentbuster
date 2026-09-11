use clap::{Args, Parser, Subcommand, ValueEnum};
use commentbuster::analyze::{analyze, resolve_mode};
use commentbuster::gate::{self, Thresholds};
use commentbuster::git;
use commentbuster::hook::{self, Decision};
use commentbuster::report::{self, Report};
use std::io::{IsTerminal, Write};
use std::process::ExitCode;
use std::time::Instant;

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum Color {
    Auto,
    Always,
    Never,
}

/// Count net comment lines added in a git diff.
#[derive(Parser)]
#[command(
    name = "commentbuster",
    version,
    about = "Count net comment lines added in a git diff (Java, Kotlin, TypeScript)",
    args_conflicts_with_subcommands = true,
    after_help = "\
REFS
  commentbuster                  working tree vs HEAD
  commentbuster --staged         index vs HEAD (the pre-commit case)
  commentbuster <ref>            <ref>...HEAD, from the merge base
  commentbuster <a> <b>          <a>...<b>, from the merge base
  commentbuster --direct <a> <b> <a>..<b>, compared literally

EXIT CODES
  0  no gate breached, or no gate configured
  1  a configured gate was breached
  2  usage error, not a git repo, bad ref, or internal failure

HOOKS
  commentbuster claude-gate      Claude Code PreToolUse hook; see its --help"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Refs to compare: none, <ref>, or <base> <head>
    #[arg(value_name = "REF", num_args = 0..=2)]
    refs: Vec<String>,

    /// Compare the index against HEAD
    #[arg(long)]
    staged: bool,

    /// Compare refs literally (two-dot) instead of from their merge base
    #[arg(long)]
    direct: bool,

    /// Human-readable output instead of JSON
    #[arg(long)]
    pretty: bool,

    /// Fail if net comment lines added exceeds N
    #[arg(long, value_name = "N", allow_negative_numbers = true)]
    max_net: Option<i64>,

    /// Fail if the comment-to-code ratio exceeds P (a fraction, e.g. 0.05)
    #[arg(long, value_name = "P")]
    max_ratio: Option<f64>,

    /// Fail if net comment lines added exceeds N, on diffs adding fewer than --min-code-lines code lines
    #[arg(long, value_name = "N", allow_negative_numbers = true)]
    max_net_small: Option<i64>,

    /// Do not apply --max-ratio to diffs adding fewer than N code lines; --max-net-small applies only there
    #[arg(long, value_name = "N", default_value_t = 20)]
    min_code_lines: u64,

    /// Show only the N worst files in --pretty output; 0 for all
    #[arg(long, value_name = "N", default_value_t = 20)]
    top: usize,

    /// When to colourise --pretty output
    #[arg(long, value_enum, default_value_t = Color::Auto)]
    color: Color,

    /// Limit the diff to these paths
    #[arg(last = true, value_name = "PATH")]
    paths: Vec<String>,
}

#[derive(Subcommand)]
enum Command {
    /// Claude Code PreToolUse hook: deny `git commit` when the staged diff is over budget
    ///
    /// Reads the pending tool call from stdin. Anything but a `git commit` is
    /// allowed untouched. A commit is checked as index vs HEAD with two
    /// complementary rules: at or above --min-code-lines added code lines the
    /// ratio decides, below it the absolute net decides. A breach prints a
    /// PreToolUse deny decision the model can act on; everything else, including
    /// the gate's own failures, prints nothing. The exit code is always 0.
    ClaudeGate(ClaudeGate),
}

#[derive(Args)]
#[command(after_help = "\
SETTINGS
  Wire into .claude/settings.json (team-wide) or .claude/settings.local.json:

    \"hooks\": { \"PreToolUse\": [{ \"matcher\": \"Bash\", \"hooks\": [
      { \"type\": \"command\", \"command\": \"$HOME/.local/bin/commentbuster claude-gate\" }
    ]}]}

  Flags win over the environment variables. COMMENTBUSTER_SKIP set to anything
  bypasses the gate.")]
struct ClaudeGate {
    /// Ratio ceiling for commits adding at least --min-code-lines code lines
    #[arg(long, value_name = "P", env = "COMMENTBUSTER_MAX_RATIO", default_value_t = 0.15)]
    max_ratio: f64,

    /// Net ceiling for commits adding fewer than --min-code-lines code lines
    #[arg(
        long,
        value_name = "N",
        env = "COMMENTBUSTER_MAX_NET_SMALL",
        default_value_t = 5,
        allow_negative_numbers = true
    )]
    max_net_small: i64,

    /// Boundary between the two rules, in added code lines
    #[arg(long, value_name = "N", env = "COMMENTBUSTER_MIN_CODE_LINES", default_value_t = 20)]
    min_code_lines: u64,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match &cli.command {
        Some(Command::ClaudeGate(args)) => claude_gate(args),
        None => run(&cli),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("commentbuster: {e:#}");
            ExitCode::from(2)
        }
    }
}

/// Ratios above 1 are legitimate: a diff that adds mostly comment lines has
/// more comment lines than code lines.
fn check_ratio(p: f64) -> anyhow::Result<()> {
    if p < 0.0 || !p.is_finite() {
        anyhow::bail!("--max-ratio takes a non-negative fraction, got {p}");
    }
    Ok(())
}

fn run(cli: &Cli) -> anyhow::Result<ExitCode> {
    let started = Instant::now();

    if let Some(p) = cli.max_ratio {
        check_ratio(p)?;
    }

    let root = git::repo_root()?;
    let mode = resolve_mode(&cli.refs, cli.staged, cli.direct)?;
    let analysis = analyze(&root, &mode, &cli.paths)?;

    let thresholds = Thresholds {
        max_net: cli.max_net,
        max_ratio: cli.max_ratio,
        max_net_small: cli.max_net_small,
        min_code_lines: cli.min_code_lines,
    };
    let gate = gate::evaluate(&analysis.totals, &thresholds);
    let breached = gate.configured && !gate.passed;

    let report = Report {
        analysis,
        gate,
        duration_ms: started.elapsed().as_millis() as u64,
    };

    let out = if cli.pretty {
        let color = match cli.color {
            Color::Always => true,
            Color::Never => false,
            Color::Auto => std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
        };
        report::pretty(&report, &mode, color, cli.top)
    } else {
        format!("{}\n", report::json(&report))
    };

    let code = if breached { ExitCode::from(1) } else { ExitCode::SUCCESS };

    // A closed consumer (`commentbuster … | head`, `| jq .totals`) is not an
    // error. Without this, writing to the dead pipe panics and prints a Rust
    // backtrace where a quiet exit belongs.
    let mut stdout = std::io::stdout().lock();
    match stdout.write_all(out.as_bytes()).and_then(|()| stdout.flush()) {
        Ok(()) => Ok(code),
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(code),
        Err(e) => Err(e.into()),
    }
}

/// Exit 0 on every path but a usage error. Claude Code reads exit 2 as "block
/// and show stderr to the model", so a misconfigured flag blocks loudly, while
/// a missing repo or an unreadable payload allows: the gate's own trouble must
/// not stop a commit.
fn claude_gate(args: &ClaudeGate) -> anyhow::Result<ExitCode> {
    check_ratio(args.max_ratio)?;
    if std::env::var_os("COMMENTBUSTER_SKIP").is_some() {
        return Ok(ExitCode::SUCCESS);
    }

    let payload = std::io::read_to_string(std::io::stdin()).unwrap_or_default();
    let thresholds = Thresholds {
        max_net: None,
        max_ratio: Some(args.max_ratio),
        max_net_small: Some(args.max_net_small),
        min_code_lines: args.min_code_lines,
    };

    match hook::decide(&payload, &thresholds) {
        Ok(Decision::Allow) => {}
        Ok(Decision::Deny(reason)) => println!("{}", hook::deny_output(&reason)),
        Err(e) => eprintln!("commentbuster claude-gate: {e:#}; allowing the commit"),
    }
    Ok(ExitCode::SUCCESS)
}
