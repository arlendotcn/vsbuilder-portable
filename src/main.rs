mod build;
mod extract;
mod model;
mod net;
mod package;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use serde_json::json;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "vsbuilder",
    version,
    about = "Portable, reproducible MSVC Build Tools / Windows SDK package builder"
)]
struct Cli {
    /// Ignore cached manifests and refresh them
    #[arg(long, global = true)]
    refresh: bool,

    /// Optional HTTP proxy, e.g. http://127.0.0.1:1080
    #[arg(long, global = true)]
    proxy: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List available MSVC and Windows SDK versions
    List(ListArgs),
    /// Download and build a candidate msvc package
    Build(BuildArgs),
    /// Read-only validation of an existing candidate package
    Check(CheckArgs),
}

#[derive(Args)]
struct ListArgs {
    /// Visual Studio channel: 2019, 2022 or 2026
    #[arg(long, default_value = "2022")]
    vs: String,
    /// Use the Preview channel
    #[arg(long)]
    preview: bool,
    /// Machine-readable output
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct BuildArgs {
    /// Visual Studio channel: 2019, 2022 or 2026
    #[arg(long, default_value = "2022")]
    vs: String,
    /// Use the Preview channel
    #[arg(long)]
    preview: bool,
    /// MSVC version: minor (14.44), full (14.44.35229) or package stem
    #[arg(long)]
    msvc: String,
    /// Windows SDK version: short (26100) or package version
    #[arg(long)]
    sdk: String,
    /// Empty output directory for the candidate package
    #[arg(long)]
    out: PathBuf,
    /// Explicitly accept the Microsoft Build Tools license
    #[arg(long)]
    accept_license: bool,
    /// Resolve and print the plan without downloading
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args)]
struct CheckArgs {
    /// Candidate package directory
    dir: PathBuf,
    /// Run cl.exe /Bv after file checks
    #[arg(long)]
    smoke: bool,
    /// Machine-readable output
    #[arg(long)]
    json: bool,
}

fn main() {
    let cli = Cli::parse();
    let result = run(cli);
    if let Err(err) = result {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    let http = net::Http::new(default_cache_dir(), cli.refresh, cli.proxy.as_deref())?;
    match cli.command {
        Command::List(args) => list(&http, args),
        Command::Build(args) => {
            let opts = build::BuildOptions {
                vs: args.vs,
                preview: args.preview,
                msvc: args.msvc,
                sdk: args.sdk,
                out: args.out,
                accept_license: args.accept_license,
                dry_run: args.dry_run,
            };
            build::build(&http, &opts)
        }
        Command::Check(args) => check(args),
    }
}

fn default_cache_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("cache")
}

fn list(http: &net::Http, args: ListArgs) -> Result<()> {
    let channel_url = model::channel_url(&args.vs, args.preview)?;
    let channel_name = format!(
        "channel-vs{}-{}.json",
        args.vs,
        if args.preview { "preview" } else { "release" }
    );
    let channel: model::ChannelManifest = http.fetch_json(channel_url, &channel_name)?;
    let manifest_url = model::manifest_url(&channel, args.preview)?;
    let vs_name = format!(
        "vs-{}-{}.json",
        args.vs,
        if args.preview { "preview" } else { "release" }
    );
    let vsmanifest: model::VsManifest = http.fetch_json(&manifest_url, &vs_name)?;
    let catalog = model::Catalog::new(vsmanifest.packages);
    let msvc = catalog.msvc_versions();
    let sdk = catalog.sdk_versions()?;

    if args.json {
        let out = json!({
            "vs": args.vs,
            "channel": if args.preview { "preview" } else { "release" },
            "msvc": msvc.iter().map(|v| json!({
                "minor": v.minor,
                "version": v.full,
                "stem": v.stem,
            })).collect::<Vec<_>>(),
            "sdk": sdk.iter().map(|v| json!({
                "short": v.short,
                "version": v.package_version,
                "component": v.component_id,
                "package": v.package_id,
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("MSVC versions ({}):", if args.preview { "preview" } else { "release" });
        for v in &msvc {
            println!("  {:<14} {:>14}  {}", v.minor, v.full, v.stem);
        }
        println!("Windows SDK versions ({}):", if args.preview { "preview" } else { "release" });
        for v in &sdk {
            println!("  {:<8} {:>16}  {}", v.short, v.package_version, v.component_id);
        }
    }
    Ok(())
}

fn check(args: CheckArgs) -> Result<()> {
    let dir = args
        .dir
        .canonicalize()
        .with_context(|| format!("resolving {}", args.dir.display()))?;
    let errors = package::check(&dir, args.smoke)?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "dir": dir,
                "ok": errors.is_empty(),
                "errors": errors,
            }))?
        );
    } else if errors.is_empty() {
        println!("OK  {}", dir.display());
    } else {
        for e in &errors {
            eprintln!("FAIL  {e}");
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        anyhow::bail!("{} check(s) failed", errors.len())
    }
}


