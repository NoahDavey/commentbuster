use clap::{Parser, ValueEnum};
use commentbuster::analyze::{analyze, resolve_mode};
use commentbuster::gate::{self, Thresholds};
use commentbuster::git;
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
  2  usage error, not a git repo, bad ref, or internal failure"
)]
struct Cli {
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

    /// Do not apply --max-ratio to diffs adding fewer than N code lines
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

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("commentbuster: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: &Cli) -> anyhow::Result<ExitCode> {
    let started = Instant::now();

    // Ratios above 1 are legitimate: a diff that adds mostly comment lines has
    // more comment lines than code lines.
    if let Some(p) = cli.max_ratio {
        if p < 0.0 || !p.is_finite() {
            anyhow::bail!("--max-ratio takes a non-negative fraction, got {p}");
        }
    }

    let root = git::repo_root()?;
    let mode = resolve_mode(&cli.refs, cli.staged, cli.direct)?;
    let analysis = analyze(&root, &mode, &cli.paths)?;

    let thresholds = Thresholds {
        max_net: cli.max_net,
        max_ratio: cli.max_ratio,
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
