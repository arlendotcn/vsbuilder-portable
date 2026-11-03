mod build;
mod extract;
mod model;
mod net;
mod package;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde_json::json;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Channel {
    Stable,
    Insiders,
    Lts,
}

impl Channel {
    fn slug(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Insiders => "insiders",
            Self::Lts => "lts",
        }
    }

    fn is_insiders(self) -> bool {
        self == Self::Insiders
    }
}

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
    #[command(after_help = "Examples:\n  vsbuilder build\n  vsbuilder build --msvc 14.44 --sdk 26100 --out D:\\builds\\msvc --accept-license\n\nWith no options, build uses the latest stable MSVC/SDK and writes to\n<exe-dir>\\msvc-<vs> (for example <exe-dir>\\msvc-2022). Without\n--accept-license it prompts when stdin is a TTY; non-interactive use requires\n--accept-license.")]
    Build(BuildArgs),
    /// Read-only validation of an existing candidate package
    Check(CheckArgs),
}

#[derive(Args)]
struct ListArgs {
    /// Visual Studio version: 2019, 2022 or 2026
    #[arg(long, default_value = "2022")]
    vs: String,
    /// Release channel: stable, insiders, or lts (lts is not available yet)
    #[arg(long, value_enum, default_value = "stable")]
    channel: Channel,
    /// Deprecated alias for --channel insiders
    #[arg(long)]
    preview: bool,
    /// Machine-readable output
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct BuildArgs {
    /// Visual Studio version: 2019, 2022 or 2026
    #[arg(long, default_value = "2022", value_name = "VERSION")]
    vs: String,
    /// Release channel: stable, insiders, or lts (lts is not available yet)
    #[arg(long, value_enum, default_value = "stable")]
    channel: Channel,
    /// Deprecated alias for --channel insiders
    #[arg(long)]
    preview: bool,
    /// MSVC version: latest, minor (14.44), full (14.44.35229), or package stem (14.44.17.14)
    #[arg(long, default_value = "latest", value_name = "VERSION")]
    msvc: String,
    /// Windows SDK version: latest, short (26100), or package version (10.0.26100.15)
    #[arg(long, default_value = "latest", value_name = "VERSION")]
    sdk: String,
    /// Output directory; must be missing or empty (default: <exe-dir>\msvc-<vs>)
    #[arg(long, value_name = "DIR")]
    out: Option<PathBuf>,
    /// Accept the Microsoft Build Tools license without prompting
    #[arg(long)]
    accept_license: bool,
    /// Resolve and print the plan without downloading or writing
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args)]
struct CheckArgs {
    /// Candidate package directory
    #[arg(value_name = "DIR")]
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
            let channel = resolve_channel(args.channel, args.preview)?;
            let out = args
                .out
                .unwrap_or_else(|| default_output_dir(&args.vs, channel));
            let opts = build::BuildOptions {
                vs: args.vs,
                channel,
                msvc: args.msvc,
                sdk: args.sdk,
                out,
                accept_license: args.accept_license,
                dry_run: args.dry_run,
            };
            build::build(&http, &opts)
        }
        Command::Check(args) => check(args),
    }
}

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn default_cache_dir() -> PathBuf {
    exe_dir().join("cache")
}

fn default_output_dir(vs: &str, channel: Channel) -> PathBuf {
    let name = if channel.is_insiders() {
        format!("msvc-{vs}-preview")
    } else if channel == Channel::Lts {
        format!("msvc-{vs}-lts")
    } else {
        format!("msvc-{vs}")
    };
    exe_dir().join(name)
}

fn resolve_channel(channel: Channel, preview: bool) -> Result<Channel> {
    if preview {
        if channel == Channel::Stable {
            return Ok(Channel::Insiders);
        }
        bail!(
            "--preview conflicts with --channel {}",
            channel.slug()
        );
    }
    Ok(channel)
}

fn list(http: &net::Http, args: ListArgs) -> Result<()> {
    let channel = resolve_channel(args.channel, args.preview)?;
    let channel_url = model::channel_url(&args.vs, channel.slug())?;
    let channel_name = format!("channel-vs{}-{}.json", args.vs, channel.slug());
    let channel_manifest: model::ChannelManifest = http.fetch_json(channel_url, &channel_name)?;
    let manifest_url = model::manifest_url(&channel_manifest, channel.is_insiders())?;
    let vs_name = format!("vs-{}-{}.json", args.vs, channel.slug());
    let vsmanifest: model::VsManifest = http.fetch_json(&manifest_url, &vs_name)?;
    let catalog = model::Catalog::new(vsmanifest.packages);
    let msvc = catalog.msvc_versions();
    let sdk = catalog.sdk_versions()?;

    if args.json {
        let out = json!({
            "vs": args.vs,
            "channel": channel.slug(),
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
        println!("MSVC versions ({}):", channel.slug());
        for v in &msvc {
            println!("  {:<14} {:>14}  {}", v.minor, v.full, v.stem);
        }
        println!("Windows SDK versions ({}):", channel.slug());
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




