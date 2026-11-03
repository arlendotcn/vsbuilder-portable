use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct PayloadRecord {
    pub file_name: String,
    pub sha256: String,
    pub url: String,
}

pub struct PackageMeta {
    pub vs: String,
    pub channel: String,
    pub requested_msvc: String,
    pub requested_sdk: String,
    pub license: String,
    pub msvc_stem: String,
    pub msvc_version: String,
    pub msvc_package: String,
    pub sdk_short: String,
    pub sdk_version: String,
    pub sdk_component: String,
    pub sdk_package: String,
    pub payloads: Vec<PayloadRecord>,
    pub built_at_unix: u64,
}

pub fn write_package(out: &Path, meta: &PackageMeta) -> Result<()> {
    let pkg = json!({
        "schemaVersion": 1,
        "id": "msvc",
        "name": "MSVC Build Tools",
        "version": meta.msvc_version,
        "priority": 60,
        "detail": format!("MSVC {} x64, Windows SDK {}", meta.msvc_version, meta.sdk_version),
        "requires": [],
        "payload": "payload",
        "dataDirs": [],
        "path": [
            format!("payload/VC/Tools/MSVC/{}/bin/Hostx64/x64", meta.msvc_version),
            format!("payload/Windows Kits/10/bin/{}/x64", meta.sdk_version),
            format!("payload/Windows Kits/10/bin/{}/x64/ucrt", meta.sdk_version)
        ],
        "env": {
            "WindowsSdkDir": "{pkg}\\payload\\Windows Kits\\10",
            "WindowsSDKVersion": format!("{}\\", meta.sdk_version),
            "VCToolsInstallDir": format!("{{pkg}}\\payload\\VC\\Tools\\MSVC\\{}\\", meta.msvc_version),
            "INCLUDE": format!(
                "{{pkg}}\\payload\\VC\\Tools\\MSVC\\{}\\include;{{pkg}}\\payload\\Windows Kits\\10\\Include\\{}\\ucrt;{{pkg}}\\payload\\Windows Kits\\10\\Include\\{}\\shared;{{pkg}}\\payload\\Windows Kits\\10\\Include\\{}\\um;{{pkg}}\\payload\\Windows Kits\\10\\Include\\{}\\winrt;{{pkg}}\\payload\\Windows Kits\\10\\Include\\{}\\cppwinrt",
                meta.msvc_version, meta.sdk_version, meta.sdk_version, meta.sdk_version, meta.sdk_version, meta.sdk_version
            ),
            "LIB": format!(
                "{{pkg}}\\payload\\VC\\Tools\\MSVC\\{}\\lib\\x64;{{pkg}}\\payload\\Windows Kits\\10\\Lib\\{}\\ucrt\\x64;{{pkg}}\\payload\\Windows Kits\\10\\Lib\\{}\\um\\x64",
                meta.msvc_version, meta.sdk_version, meta.sdk_version
            ),
            "VSCMD_ARG_TGT_ARCH": "x64",
            "VSCMD_ARG_HOST_ARCH": "x64"
        },
        "probe": [
            format!("payload/VC/Tools/MSVC/{}/bin/Hostx64/x64/cl.exe", meta.msvc_version)
        ]
    });
    fs::write(
        out.join("pkg.json"),
        serde_json::to_vec_pretty(&pkg).context("serializing pkg.json")?,
    )?;

    let info = json!({
        "tool": "vsbuilder",
        "toolVersion": env!("CARGO_PKG_VERSION"),
        "vs": meta.vs,
        "channel": meta.channel,
        "requested": {
            "msvc": meta.requested_msvc,
            "sdk": meta.requested_sdk
        },
        "license": meta.license,
        "msvc": {
            "version": meta.msvc_version,
            "stem": meta.msvc_stem,
            "package": meta.msvc_package
        },
        "sdk": {
            "short": meta.sdk_short,
            "version": meta.sdk_version,
            "component": meta.sdk_component,
            "package": meta.sdk_package
        },
        "payloads": meta.payloads.iter().map(|p| json!({
            "fileName": p.file_name,
            "sha256": p.sha256,
            "url": p.url
        })).collect::<Vec<_>>(),
        "builtAtUnix": meta.built_at_unix
    });
    fs::write(
        out.join("BUILD-INFO.json"),
        serde_json::to_vec_pretty(&info).context("serializing BUILD-INFO.json")?,
    )?;

    write_devcmd(out, &meta.msvc_version, &meta.sdk_version)?;
    Ok(())
}

fn write_devcmd(out: &Path, msvc: &str, sdk: &str) -> Result<()> {
    let payload = out.join("payload");
    let bat = format!(
        "@echo off\r\n\
set \"BUILD_TOOLS_ROOT=%~dp0\"\r\n\
set \"WindowsSDKDir=%BUILD_TOOLS_ROOT%Windows Kits\\10\"\r\n\
set \"VCToolsInstallDir=%BUILD_TOOLS_ROOT%VC\\Tools\\MSVC\\{msvc}\"\r\n\
set \"WindowsSDKVersion={sdk}\"\r\n\
set \"VSCMD_ARG_TGT_ARCH=x64\"\r\n\
set \"VSCMD_ARG_HOST_ARCH=x64\"\r\n\
set \"INCLUDE=%VCToolsInstallDir%\\include;%WindowsSDKDir%\\Include\\%WindowsSDKVersion%\\ucrt;%WindowsSDKDir%\\Include\\%WindowsSDKVersion%\\shared;%WindowsSDKDir%\\Include\\%WindowsSDKVersion%\\um;%WindowsSDKDir%\\Include\\%WindowsSDKVersion%\\winrt;%WindowsSDKDir%\\Include\\%WindowsSDKVersion%\\cppwinrt\"\r\n\
set \"LIB=%VCToolsInstallDir%\\lib\\x64;%WindowsSDKDir%\\Lib\\%WindowsSDKVersion%\\ucrt\\x64;%WindowsSDKDir%\\Lib\\%WindowsSDKVersion%\\um\\x64\"\r\n\
set \"BUILD_TOOLS_BIN=%VCToolsInstallDir%\\bin\\Hostx64\\x64;%WindowsSDKDir%\\bin\\%WindowsSDKVersion%\\x64;%WindowsSDKDir%\\bin\\%WindowsSDKVersion%\\x64\\ucrt\"\r\n\
set \"PATH=%BUILD_TOOLS_BIN%;%PATH%\"\r\n"
    );
    fs::write(payload.join("devcmd.bat"), bat)?;

    let ps1 = format!(
        "#Requires -Version 5\r\n\
param([string]$InstallPath = $PSScriptRoot)\r\n\
$env:BUILD_TOOLS_ROOT = $InstallPath\r\n\
$env:WindowsSDKDir = Join-Path $InstallPath 'Windows Kits\\10'\r\n\
$env:VCToolsInstallDir = Join-Path $InstallPath 'VC\\Tools\\MSVC\\{msvc}'\r\n\
$env:WindowsSDKVersion = '{sdk}'\r\n\
$env:VSCMD_ARG_TGT_ARCH = 'x64'\r\n\
$env:VSCMD_ARG_HOST_ARCH = 'x64'\r\n\
$env:INCLUDE = \"$env:VCToolsInstallDir\\include;$env:WindowsSDKDir\\Include\\$env:WindowsSDKVersion\\ucrt;$env:WindowsSDKDir\\Include\\$env:WindowsSDKVersion\\shared;$env:WindowsSDKDir\\Include\\$env:WindowsSDKVersion\\um;$env:WindowsSDKDir\\Include\\$env:WindowsSDKVersion\\winrt;$env:WindowsSDKDir\\Include\\$env:WindowsSDKVersion\\cppwinrt\"\r\n\
$env:LIB = \"$env:VCToolsInstallDir\\lib\\x64;$env:WindowsSDKDir\\Lib\\$env:WindowsSDKVersion\\ucrt\\x64;$env:WindowsSDKDir\\Lib\\$env:WindowsSDKVersion\\um\\x64\"\r\n\
$env:BUILD_TOOLS_BIN = \"$env:VCToolsInstallDir\\bin\\Hostx64\\x64;$env:WindowsSDKDir\\bin\\$env:WindowsSDKVersion\\x64;$env:WindowsSDKDir\\bin\\$env:WindowsSDKVersion\\x64\\ucrt\"\r\n\
$env:PATH = \"$env:BUILD_TOOLS_BIN;$env:PATH\"\r\n\
'Portable Build Tools environment started.'\r\n"
    );
    fs::write(payload.join("devcmd.ps1"), ps1)?;
    Ok(())
}

pub fn read_pkg(dir: &Path) -> Result<Value> {
    let path = dir.join("pkg.json");
    let bytes = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("invalid {}", path.display()))
}

pub fn check(dir: &Path, smoke: bool) -> Result<Vec<String>> {
    let mut errors = Vec::new();
    let pkg = read_pkg(dir)?;
    if pkg.get("id").and_then(Value::as_str) != Some("msvc") {
        errors.push("pkg.json id is not 'msvc'".to_string());
    }
    let version = pkg
        .get("version")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("pkg.json version missing"))?
        .to_string();
    let payload = dir.join("payload");
    let cl = payload
        .join("VC")
        .join("Tools")
        .join("MSVC")
        .join(&version)
        .join("bin")
        .join("Hostx64")
        .join("x64")
        .join("cl.exe");
    if !cl.exists() {
        errors.push(format!("missing {}", cl.display()));
    }
    for f in ["devcmd.bat", "devcmd.ps1"] {
        let p = payload.join(f);
        if !p.exists() {
            errors.push(format!("missing {}", p.display()));
        }
    }
    let info_path = dir.join("BUILD-INFO.json");
    if !info_path.exists() {
        errors.push("missing BUILD-INFO.json".to_string());
    } else if let Ok(info) = fs::read(&info_path)
        .map_err(anyhow::Error::from)
        .and_then(|b| serde_json::from_slice::<Value>(&b).map_err(anyhow::Error::from))
    {
        if info.get("msvc").and_then(|v| v.get("version")).and_then(Value::as_str) != Some(version.as_str()) {
            errors.push("BUILD-INFO msvc.version does not match pkg.json".to_string());
        }
    } else {
        errors.push("invalid BUILD-INFO.json".to_string());
    }
    let sdk_bin = payload.join("Windows Kits").join("10").join("bin");
    if !sdk_bin.exists() {
        errors.push(format!("missing {}", sdk_bin.display()));
    } else {
        let has_x64 = fs::read_dir(&sdk_bin)?
            .filter_map(|e| e.ok())
            .any(|e| e.path().join("x64").exists());
        if !has_x64 {
            errors.push(format!("no SDK x64 bin under {}", sdk_bin.display()));
        }
    }
    if let Some(env) = pkg.get("env").and_then(Value::as_object) {
        for key in ["INCLUDE", "LIB", "WindowsSdkDir", "VCToolsInstallDir"] {
            if let Some(value) = env.get(key).and_then(Value::as_str) {
                for part in value.split(';') {
                    let part = part.replace("{pkg}", &dir.display().to_string());
                    if part.is_empty() {
                        continue;
                    }
                    let p = PathBuf::from(part.trim_end_matches('\\'));
                    if !p.exists() {
                        errors.push(format!("env {key} path missing: {}", p.display()));
                    }
                }
            }
        }
    }

    if smoke && cl.exists() {
        let output = Command::new(&cl)
            .arg("/Bv")
            .output()
            .with_context(|| format!("running {}", cl.display()))?;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if !text.contains("Microsoft") {
            errors.push("cl.exe smoke output did not look like MSVC".to_string());
        }
    }
    Ok(errors)
}

pub fn sdk_version_from_payload(payload: &Path) -> Result<String> {
    let bin = payload.join("Windows Kits").join("10").join("bin");
    let mut versions: Vec<String> = fs::read_dir(&bin)
        .with_context(|| format!("reading {}", bin.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    versions.sort_by(|a, b| crate::model::version_cmp(a, b));
    versions
        .pop()
        .ok_or_else(|| anyhow!("no Windows SDK version directory under {}", bin.display()))
}

pub fn msvc_version_from_payload(payload: &Path) -> Result<String> {
    let root = payload.join("VC").join("Tools").join("MSVC");
    let mut versions: Vec<String> = fs::read_dir(&root)
        .with_context(|| format!("reading {}", root.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    versions.sort_by(|a, b| crate::model::version_cmp(a, b));
    versions
        .pop()
        .ok_or_else(|| anyhow!("no MSVC version directory under {}", root.display()))
}

pub fn ensure_empty_output(out: &Path) -> Result<()> {
    if out.exists() {
        if !out.is_dir() {
            bail!("output exists and is not a directory: {}", out.display());
        }
        if fs::read_dir(out)?.next().is_some() {
            bail!("output directory is not empty: {}", out.display());
        }
    }
    Ok(())
}


