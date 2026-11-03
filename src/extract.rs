use anyhow::{bail, Context, Result};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

pub fn extract_zip_contents(zip_path: &Path, output: &Path) -> Result<usize> {
    let file = File::open(zip_path)
        .with_context(|| format!("opening zip {}", zip_path.display()))?;
    let mut zip = zip::ZipArchive::new(file)
        .with_context(|| format!("reading zip {}", zip_path.display()))?;

    let mut has_contents = false;
    for i in 0..zip.len() {
        let name = zip.by_index(i)?.name().replace('\\', "/");
        if name.starts_with("Contents/") {
            has_contents = true;
            break;
        }
    }

    let mut count = 0usize;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let raw = entry.name().replace('\\', "/");
        let name = if has_contents {
            match raw.strip_prefix("Contents/") {
                Some(n) => n,
                None => continue,
            }
        } else {
            raw.as_str()
        };
        if name.is_empty() || name.ends_with('/') {
            continue;
        }
        let Some(rel) = safe_rel(name) else {
            continue;
        };
        let out = output.join(rel);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut f = File::create(&out)
            .with_context(|| format!("creating {}", out.display()))?;
        std::io::copy(&mut entry, &mut f)?;
        count += 1;
    }
    if count == 0 {
        bail!("zip {} contained no extractable files", zip_path.display());
    }
    Ok(count)
}

fn safe_rel(name: &str) -> Option<PathBuf> {
    let normalized = name.replace('\\', "/");
    let mut out = PathBuf::new();
    for component in Path::new(&normalized).components() {
        match component {
            Component::Normal(c) => out.push(c),
            Component::CurDir => {}
            _ => return None,
        }
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

pub fn scan_cab_names(msi: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    if msi.len() < 36 {
        return names;
    }
    for i in 32..msi.len().saturating_sub(3) {
        if &msi[i..i + 4] == b".cab" {
            let start = i - 32;
            let raw = String::from_utf8_lossy(&msi[start..i + 4]);
            let name = raw.trim_matches('\0').trim().to_string();
            if name.len() > 4
                && name.to_ascii_lowercase().ends_with(".cab")
                && !name.contains('\u{fffd}')
                && !names.iter().any(|n| n.eq_ignore_ascii_case(&name))
            {
                names.push(name);
            }
        }
    }
    names.sort();
    names
}

pub fn extract_msi(msi: &Path, output: &Path) -> Result<()> {
    fs::create_dir_all(output)?;
    let target = format!("TARGETDIR={}", output.display());
    let status = Command::new("msiexec.exe")
        .arg("/a")
        .arg(msi)
        .arg("/quiet")
        .arg("/qn")
        .arg(&target)
        .status()
        .with_context(|| format!("running msiexec for {}", msi.display()))?;
    if !status.success() {
        bail!(
            "msiexec failed for {} with status {}",
            msi.display(),
            status
        );
    }
    Ok(())
}

pub fn remove_if_exists(path: &Path) -> Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)
            .with_context(|| format!("removing directory {}", path.display()))?;
    } else if path.exists() {
        fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
    }
    Ok(())
}

pub fn version_dirs(parent: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs: Vec<_> = fs::read_dir(parent)
        .with_context(|| format!("reading {}", parent.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    Ok(dirs)
}

pub fn copy_debug_crt(payload: &Path, msvc_version: &str) -> Result<()> {
    let redist = payload.join("VC").join("Redist").join("MSVC");
    if !redist.exists() {
        return Ok(());
    }
    let Some(version_dir) = version_dirs(&redist)?.into_iter().next() else {
        return Ok(());
    };
    let src = version_dir.join("debug_nonredist").join("x64");
    if !src.exists() {
        return Ok(());
    }
    let dst = payload
        .join("VC")
        .join("Tools")
        .join("MSVC")
        .join(msvc_version)
        .join("bin")
        .join("Hostx64")
        .join("x64");
    fs::create_dir_all(&dst)?;
    copy_dlls_flat(&src, &dst)?;
    Ok(())
}

fn copy_dlls_flat(src: &Path, dst: &Path) -> Result<()> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            copy_dlls_flat(&path, dst)?;
        } else if path
            .extension()
            .map(|e| e.to_string_lossy().eq_ignore_ascii_case("dll"))
            .unwrap_or(false)
        {
            let to = dst.join(path.file_name().unwrap());
            fs::copy(&path, &to)
                .with_context(|| format!("copying {} to {}", path.display(), to.display()))?;
        }
    }
    Ok(())
}

pub fn copy_msdia(payload: &Path, msvc_version: &str) -> Result<()> {
    let src = payload
        .join("DIA%20SDK")
        .join("bin")
        .join("amd64")
        .join("msdia140.dll");
    if !src.exists() {
        return Ok(());
    }
    let dst = payload
        .join("VC")
        .join("Tools")
        .join("MSVC")
        .join(msvc_version)
        .join("bin")
        .join("Hostx64")
        .join("x64")
        .join("msdia140.dll");
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(&src, &dst)
        .with_context(|| format!("copying {} to {}", src.display(), dst.display()))?;
    Ok(())
}

pub fn prune_msvc(payload: &Path, msvc_version: &str) -> Result<()> {
    let msvc = payload.join("VC").join("Tools").join("MSVC").join(msvc_version);
    let _ = remove_if_exists(&msvc.join("Auxiliary"));
    let _ = remove_if_exists(&msvc.join("bin").join("Hostx64").join("x64").join("onecore"));
    for sub in ["store", "uwp", "enclave", "onecore"] {
        let _ = remove_if_exists(&msvc.join("lib").join("x64").join(sub));
    }
    let _ = remove_if_exists(
        &msvc
            .join("bin")
            .join("Hostx64")
            .join("x64")
            .join("vctip.exe"),
    );
    Ok(())
}

pub fn prune_sdk(payload: &Path, sdk_version: &str) -> Result<()> {
    let sdk = payload.join("Windows Kits").join("10");
    for sub in ["Catalogs", "DesignTime"] {
        let _ = remove_if_exists(&sdk.join(sub));
    }
    let _ = remove_if_exists(&sdk.join("bin").join(sdk_version).join("chpe"));
    let _ = remove_if_exists(
        &sdk
            .join("Lib")
            .join(sdk_version)
            .join("ucrt_enclave"),
    );
    for arch in ["x86", "arm", "arm64"] {
        let _ = remove_if_exists(&sdk.join("Lib").join(sdk_version).join("ucrt").join(arch));
        let _ = remove_if_exists(&sdk.join("Lib").join(sdk_version).join("um").join(arch));
        let _ = remove_if_exists(&sdk.join("bin").join(sdk_version).join(arch));
    }
    Ok(())
}

pub fn read_all(path: &Path) -> Result<Vec<u8>> {
    let mut f = File::open(path)?;
    let mut out = Vec::new();
    f.read_to_end(&mut out)?;
    Ok(out)
}



