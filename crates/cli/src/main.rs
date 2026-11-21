//! `disk-cleaner` command line. Phase 1 is read-only: `scan` and `plan` never change anything
//! except the output file named with `--out`.

mod report;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use disk_cleaner_core::{
    config, plan, read_scan, resolve_active, running_cwds, scan, write_json, ConfigSource,
    FactsConfig, Git, PlanOptions, ScanOptions,
};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Parser)]
#[command(
    name = "disk-cleaner",
    version,
    about = "Understand what fills a disk. Read-only: scans and dry-run plans only."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scan directories, classify them and gather repo safety facts.
    Scan(ScanArgs),
    /// Turn a scan into a dry-run plan. Nothing is changed.
    Plan(PlanArgs),
}

#[derive(Args)]
struct ScanArgs {
    /// Directories to scan.
    #[arg(required = true)]
    paths: Vec<PathBuf>,
    /// Print the scan as JSON on stdout instead of a summary.
    #[arg(long)]
    json: bool,
    /// Also write the scan JSON to this file (the only file written).
    #[arg(long, value_name = "FILE")]
    out: Option<PathBuf>,
    /// Do not cross mount points (Unix).
    #[arg(long)]
    one_file_system: bool,
    /// How many of the biggest files to keep.
    #[arg(long, default_value_t = 50)]
    top_files: usize,
    /// List output files (video, archives, models...) of at least this many bytes anywhere.
    /// Inside an artifact an output is listed whatever its size, except in an installed
    /// package's directory, where this threshold applies.
    #[arg(long, default_value_t = 100 * 1024 * 1024)]
    min_output_bytes: u64,
    /// Skip git; every repo fact is then unknown.
    #[arg(long)]
    no_git: bool,
    /// Per-command git timeout in seconds. A timeout makes the fact unknown.
    #[arg(long, default_value_t = 30)]
    git_timeout_secs: u64,
    /// Number of git processes to run at once.
    #[arg(long, default_value_t = 4)]
    git_jobs: usize,
}

#[derive(Args)]
struct PlanArgs {
    /// Scan JSON produced by `scan --out` or `scan --json`.
    #[arg(long, value_name = "FILE")]
    scan: PathBuf,
    /// A path the user says is active. Repeatable. Everything under it is kept.
    #[arg(long = "active", value_name = "PATH")]
    active: Vec<PathBuf>,
    /// A project is active when touched within this many days (overrides the config file;
    /// the built-in default is 7).
    #[arg(long, value_name = "DAYS")]
    active_window_days: Option<u64>,
    /// Config file (default: <platform config dir>/disk-cleaner/config.toml).
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,
    /// Print the plan as JSON.
    #[arg(long)]
    json: bool,
    /// Accept a partial process check: when some running processes cannot be inspected, still
    /// offer removal based on the ones that could. Without this (or the config setting
    /// `trust_partial_process_check = true`) every removing action becomes `ask` then.
    #[arg(long)]
    trust_partial_process_check: bool,
    /// Accept incomplete coverage: when the scan could not read some folders or did not enter
    /// some mount points, still offer repositories for removal after a backup, each reason
    /// saying what was not read (same as the config setting `accept_incomplete_coverage =
    /// true`). Without it every repository's dependents are unknown then, so each one asks.
    #[arg(long)]
    accept_incomplete_coverage: bool,
    /// Retired: strict coverage is now the default. Refused with a message naming
    /// `--accept-incomplete-coverage`.
    #[arg(long, hide = true)]
    require_full_coverage: bool,
    /// Override the current time (unix seconds); for reproducible plans and tests.
    #[arg(long, hide = true)]
    now: Option<i64>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Scan(a) => run_scan(a),
        Command::Plan(a) => run_plan(a),
    }
}

fn run_scan(a: ScanArgs) -> Result<()> {
    let mut opts = ScanOptions::new(a.paths);
    opts.one_file_system = a.one_file_system;
    opts.top_files = a.top_files;
    opts.min_output_bytes = a.min_output_bytes;
    opts.collect_facts = !a.no_git;
    opts.git_jobs = a.git_jobs;
    let git = Git::new("git", Duration::from_secs(a.git_timeout_secs.max(1)));
    opts.facts = FactsConfig::default().with_git(git);
    let result = scan(&opts).context("scan failed")?;
    if let Some(out) = &a.out {
        write_json(out, &result).context("cannot write scan file")?;
    }
    if a.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        print!("{}", report::scan_summary(&result));
        if let Some(out) = &a.out {
            println!("\nScan written to {}", out.display());
        }
    }
    Ok(())
}

fn run_plan(a: PlanArgs) -> Result<()> {
    if a.require_full_coverage {
        anyhow::bail!(
            "--require-full-coverage is no longer an option: incomplete coverage now makes \
             repositories ask by default; drop the flag, or pass --accept-incomplete-coverage \
             (config: accept_incomplete_coverage = true) to offer them for removal despite \
             folders the scan could not read"
        );
    }
    let scan_result = read_scan(&a.scan).context("cannot read scan file")?;
    let now = a.now.unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    });
    let active = resolve_active(&scan_result, &a.active)?;
    // The config is always loaded and validated, even when a flag overrides it, so a broken
    // file is reported instead of being ignored.
    let (cfg, source) = config::load(a.config.as_deref()).context("invalid configuration")?;
    let (window, window_source) = match (a.active_window_days, source) {
        (Some(n), _) => (n, "--active-window-days".to_string()),
        (None, ConfigSource::File(p)) => {
            (cfg.active_window_days, format!("config {}", p.display()))
        }
        (None, ConfigSource::Default) => (cfg.active_window_days, "default".to_string()),
    };
    let opts = PlanOptions {
        active,
        active_window_days: window,
        window_source,
        now,
        process: running_cwds(),
        trust_partial_process_check: a.trust_partial_process_check
            || cfg.trust_partial_process_check,
        accept_incomplete_coverage: a.accept_incomplete_coverage || cfg.accept_incomplete_coverage,
        max_git_admin_bytes: cfg.max_git_admin_bytes,
    };
    let p = plan(&scan_result, &opts);
    if a.json {
        println!("{}", serde_json::to_string_pretty(&p)?);
    } else {
        print!("{}", report::plan_summary(&p));
    }
    Ok(())
}
