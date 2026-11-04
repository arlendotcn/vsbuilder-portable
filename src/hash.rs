use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone)]
pub struct TreeInfo {
    pub sha256: String,
    pub file_count: u64,
    pub total_bytes: u64,
    pub files: Vec<FileEntry>,
}

impl TreeInfo {
    pub fn manifest_tsv(&self) -> String {
        let mut out = String::new();
        for f in &self.files {
            out.push_str(&f.sha256);
            out.push('\t');
            out.push_str(&f.size.to_string());
            out.push('\t');
            out.push_str(&f.path);
            out.push('\n');
        }
        out
    }
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

pub fn hash_tree(root: &Path) -> Result<TreeInfo> {
    let mut paths = Vec::new();
    collect_files(root, root, &mut paths)?;
    paths.sort_by(|a, b| {
        let ak = a.to_string_lossy().replace('\\', "/").to_ascii_lowercase();
        let bk = b.to_string_lossy().replace('\\', "/").to_ascii_lowercase();
        ak.cmp(&bk)
    });

    let mut tree = Sha256::new();
    let mut files = Vec::with_capacity(paths.len());
    let mut total_bytes = 0u64;

    for path in paths {
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let meta = fs::metadata(&path)
            .with_context(|| format!("reading metadata {}", path.display()))?;
        if !meta.is_file() {
            continue;
        }
        total_bytes += meta.len();
        tree.update(b"F\0");
        tree.update(rel.as_bytes());
        tree.update(b"\0");
        tree.update(meta.len().to_le_bytes());

        let mut file_hash = Sha256::new();
        let mut file = File::open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            file_hash.update(&buf[..n]);
            tree.update(&buf[..n]);
        }
        files.push(FileEntry {
            path: rel,
            size: meta.len(),
            sha256: format!("{:x}", file_hash.finalize()),
        });
    }

    Ok(TreeInfo {
        sha256: format!("{:x}", tree.finalize()),
        file_count: files.len() as u64,
        total_bytes,
        files,
    })
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)
        .with_context(|| format!("reading directory {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            collect_files(root, &path, out)?;
        } else if ft.is_file() {
            out.push(path);
        }
    }
    let _ = root;
    Ok(())
}
