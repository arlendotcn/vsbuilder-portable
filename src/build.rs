use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::extract;
use crate::model::{self, Catalog, ChannelManifest, Payload, VsManifest};
use crate::net::Http;
use crate::package::{self, PackageMeta, PayloadRecord};
use crate::Channel;

pub struct BuildOptions {
    pub vs: String,
    pub channel: Channel,
    pub msvc: String,
    pub sdk: String,
    pub out: PathBuf,
    pub accept_license: bool,
    pub dry_run: bool,
}

pub fn build(http: &Http, opts: &BuildOptions) -> Result<()> {
    let channel_url = model::channel_url(&opts.vs, opts.channel.slug())?;
    let channel_name = format!("channel-vs{}-{}.json", opts.vs, opts.channel.slug());
    let channel: ChannelManifest = http.fetch_json(channel_url, &channel_name)?;
    let manifest_url = model::manifest_url(&channel, opts.channel.is_insiders())?;
    let vs_name = format!("vs-{}-{}.json", opts.vs, opts.channel.slug());
    let vsmanifest: VsManifest = http.fetch_json(&manifest_url, &vs_name)?;
    let catalog = Catalog::new(vsmanifest.packages);
    let msvc = catalog.select_msvc(&opts.msvc)?;
    let sdk = catalog.select_sdk(&opts.sdk)?;
    let license = Catalog::license_url(&channel)?;

    println!("VS        : {}", opts.vs);
    println!("Channel   : {}", opts.channel.slug());
    println!("MSVC      : {}  ({})", msvc.full, msvc.package_id);
    println!("Windows SDK: {}  ({})", sdk.package_version, sdk.package_id);
    println!("Output    : {}", opts.out.display());
    println!("License   : {license}");

    if opts.dry_run {
        println!("dry-run: nothing downloaded or written");
        return Ok(());
    }
    if !opts.accept_license {
        if !confirm_license(&license)? {
            bail!("license not accepted");
        }
    }
    package::ensure_empty_output(&opts.out)?;

    let parent = opts.out.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let temp = tempfile::Builder::new()
        .prefix(".vsbuilder-")
        .tempdir_in(parent)
        .with_context(|| format!("creating temp under {}", parent.display()))?;
    let root = temp.path().to_path_buf();
    let payload = root.join("payload");
    fs::create_dir_all(&payload)?;

    let mut records: Vec<PayloadRecord> = Vec::new();
    println!("Downloading MSVC packages...");
    let msvc_ids = msvc_package_ids(&msvc.stem);
    for id in &msvc_ids {
        let Some(pkg) = catalog.get(id) else {
            if id.contains(".tools.hostx64.targetx64.base") {
                bail!("required MSVC package missing: {id}");
            }
            eprintln!("  skip missing optional package: {id}");
            continue;
        };
        for p in &pkg.payloads {
let file = http.download_payload(p)?;
            records.push(record(p));
            extract::extract_zip_contents(&file, &payload)
                .with_context(|| format!("extracting {}", p.file_name))?;
        }
    }

    let msvc_version = package::msvc_version_from_payload(&payload)
        .context("determining extracted MSVC version")?;
    println!("Extracted MSVC: {msvc_version}");

    println!("Downloading Windows SDK packages...");
    let sdk_dir = root.join("sdk-msi");
    fs::create_dir_all(&sdk_dir)?;
    let sdk_payloads: HashMap<String, &Payload> = sdk
        .package
        .payloads
        .iter()
        .map(|p| (normalize(&p.file_name), p))
        .collect();
    let msi_names = sdk_msi_names();
    let mut msi_paths = Vec::new();
    let mut cab_names: Vec<String> = Vec::new();
    for name in &msi_names {
        let key = normalize(&format!("Installers/{name}"));
        let Some(p) = sdk_payloads.get(&key) else {
            eprintln!("  skip missing optional SDK MSI: {name}");
            continue;
        };
        let cache = http.download_payload(p)?;
        let target = sdk_dir.join(name);
        fs::copy(&cache, &target)?;
        let bytes = extract::read_all(&cache)?;
        for cab in extract::scan_cab_names(&bytes) {
            if !cab_names.iter().any(|x| x.eq_ignore_ascii_case(&cab)) {
                cab_names.push(cab);
            }
        }
        records.push(record(p));
        msi_paths.push(target);
    }
    if msi_paths.is_empty() {
        bail!("no SDK MSI payloads found for {}", sdk.short);
    }

    for cab in &cab_names {
        let key = normalize(&format!("Installers/{cab}"));
        let Some(p) = sdk_payloads.get(&key) else {
            bail!("SDK cab not found in manifest: {cab}");
        };
        let cache = http.download_payload(p)?;
        let target = sdk_dir.join(cab);
        fs::copy(&cache, &target)?;
        records.push(record(p));
    }

    println!("Extracting Windows SDK ({} MSI, {} CAB)...", msi_paths.len(), cab_names.len());
    for msi in &msi_paths {
        extract::extract_msi(msi, &payload)
            .with_context(|| format!("extracting {}", msi.display()))?;
    }
    merge_program_files(&payload)?;
    remove_root_install_files(&payload)?;

    let sdk_version = package::sdk_version_from_payload(&payload)
        .context("determining extracted Windows SDK version")?;
    println!("Extracted Windows SDK: {sdk_version}");

    println!("Post-processing payload...");
    extract::copy_debug_crt(&payload, &msvc_version)?;
    extract::copy_msdia(&payload, &msvc_version)?;
    extract::prune_msvc(&payload, &msvc_version)?;
    extract::prune_sdk(&payload, &sdk_version)?;
    for rel in ["Common7", "VC/Redist", "DIA%20SDK"] {
        let _ = extract::remove_if_exists(&payload.join(rel));
    }
    create_vcvars_stubs(&payload)?;

    let built_at_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let meta = PackageMeta {
        vs: opts.vs.clone(),
        channel: opts.channel.slug().to_string(),
        requested_msvc: opts.msvc.clone(),
        requested_sdk: opts.sdk.clone(),
        license: license.clone(),
        msvc_stem: msvc.stem.clone(),
        msvc_version,
        msvc_package: msvc.package_id.clone(),
        sdk_short: sdk.short.clone(),
        sdk_version,
        sdk_component: sdk.component_id.clone(),
        sdk_package: sdk.package_id.clone(),
        payloads: records,
        built_at_unix,
    };
    package::write_package(&root, &meta)?;

    let errors = package::check(&root, false)?;
    if !errors.is_empty() {
        for e in &errors {
            eprintln!("check: {e}");
        }
        bail!("candidate package failed validation");
    }

    if opts.out.exists() {
        fs::remove_dir(&opts.out)
            .with_context(|| format!("removing empty {}", opts.out.display()))?;
    }
    let root_path = root.clone();
    let _ = temp.keep();
    fs::rename(&root_path, &opts.out)
        .with_context(|| format!("moving candidate to {}", opts.out.display()))?;
    println!("Built {} (MSVC {}, SDK {})", opts.out.display(), meta.msvc_version, meta.sdk_version);
    Ok(())
}

fn record(p: &Payload) -> PayloadRecord {
    PayloadRecord {
        file_name: p.file_name.clone(),
        sha256: p.sha256.clone(),
        url: p.url.clone(),
    }
}

fn confirm_license(license: &str) -> Result<bool> {
    if !io::stdin().is_terminal() {
        bail!("license not accepted; pass --accept-license (license: {license})");
    }
    print!("Accept the Microsoft Build Tools license at {license}? [y/N] ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn remove_root_install_files(payload: &Path) -> Result<()> {
    for entry in fs::read_dir(payload)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
            if matches!(ext.as_deref(), Some("msi") | Some("cab")) {
                let _ = fs::remove_file(path);
            }
        }
    }
    Ok(())
}

fn normalize(name: &str) -> String {
    name.replace('\\', "/").to_ascii_lowercase()
}

fn msvc_package_ids(stem: &str) -> Vec<String> {
    let mut ids = vec!["microsoft.visualcpp.dia.sdk".to_string()];
    for suffix in [
        "crt.headers.base",
        "crt.source.base",
        "asan.headers.base",
        "pgo.headers.base",
        "tools.hostx64.targetx64.base",
        "tools.hostx64.targetx64.res.base",
        "crt.x64.desktop.base",
        "crt.x64.store.base",
        "premium.tools.hostx64.targetx64.base",
        "pgo.x64.base",
        "asan.x64.base",
        "crt.redist.x64.base",
    ] {
        ids.push(format!("microsoft.vc.{stem}.{suffix}"));
    }
    ids
}

fn sdk_msi_names() -> Vec<&'static str> {
    vec![
        "Windows SDK for Windows Store Apps Tools-x86_en-us.msi",
        "Windows SDK for Windows Store Apps Headers-x86_en-us.msi",
        "Windows SDK for Windows Store Apps Headers OnecoreUap-x86_en-us.msi",
        "Windows SDK for Windows Store Apps Libs-x86_en-us.msi",
        "Universal CRT Headers Libraries and Sources-x86_en-us.msi",
        "Windows SDK Desktop Headers x64-x86_en-us.msi",
        "Windows SDK OnecoreUap Headers x64-x86_en-us.msi",
        "Windows SDK Desktop Libs x64-x86_en-us.msi",
        "Windows SDK Desktop Tools x64-x86_en-us.msi",
    ]
}

fn merge_program_files(payload: &Path) -> Result<()> {
    let pf = payload.join("Program Files");
    if !pf.exists() {
        return Ok(());
    }
    let src = pf.join("Windows Kits");
    if src.exists() {
        let dst = payload.join("Windows Kits");
        if dst.exists() {
            copy_tree(&src, &dst)?;
        } else {
            fs::rename(&src, &dst)?;
        }
    }
    let _ = extract::remove_if_exists(&pf);
    Ok(())
}

fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            fs::create_dir_all(&to)?;
            copy_tree(&from, &to)?;
        } else {
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn create_vcvars_stubs(payload: &Path) -> Result<()> {
    let dir = payload.join("VC").join("Auxiliary").join("Build");
    fs::create_dir_all(&dir)?;
    fs::write(
        dir.join("vcvarsall.bat"),
        "rem stub for nvcc; use devcmd.bat\r\n",
    )?;
    fs::write(dir.join("vcvars64.bat"), "rem stub for nvcc; use devcmd.bat\r\n")?;
    Ok(())
}





