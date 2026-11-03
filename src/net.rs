use anyhow::{bail, Context, Result};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::model::Payload;

const MANIFEST_TTL: Duration = Duration::from_secs(6 * 60 * 60);

pub struct Http {
    agent: ureq::Agent,
    pub cache: PathBuf,
    pub refresh: bool,
}

impl Http {
    pub fn new(cache: PathBuf, refresh: bool, proxy: Option<&str>) -> Result<Self> {
        fs::create_dir_all(&cache)
            .with_context(|| format!("creating cache {} (make the vsbuilder.exe directory writable)", cache.display()))?;
        let mut builder = ureq::AgentBuilder::new()
            .timeout_read(Duration::from_secs(600))
            .user_agent("vsbuilder/0.1");
        if let Some(proxy) = proxy {
            builder = builder.proxy(
                ureq::Proxy::new(proxy).with_context(|| format!("invalid proxy {proxy}"))?,
            );
        }
        Ok(Self {
            agent: builder.build(),
            cache,
            refresh,
        })
    }

    pub fn fetch_json<T: DeserializeOwned>(&self, url: &str, cache_name: &str) -> Result<T> {
        let path = self.cache.join("manifests").join(cache_name);
        let cached = if path.exists() {
            Some(
                fs::read(&path)
                    .with_context(|| format!("reading cached manifest {}", path.display()))?,
            )
        } else {
            None
        };
        let fresh = path
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| {
                SystemTime::now()
                    .duration_since(t)
                    .map(|age| age < MANIFEST_TTL)
                    .unwrap_or(true)
            })
            .unwrap_or(false);
        if !self.refresh && fresh {
            if let Some(bytes) = &cached {
                match crate::model::parse_json(bytes, cache_name) {
                    Ok(value) => return Ok(value),
                    Err(_) => {
                        eprintln!("warning: cached manifest {cache_name} is invalid; refreshing");
                    }
                }
            }
        }
        match self.get_bytes(url, None) {
            Ok(bytes) => {
                let parsed = crate::model::parse_json(&bytes, cache_name)?;
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                let tmp = path.with_extension("tmp");
                fs::write(&tmp, &bytes)?;
                fs::rename(&tmp, &path)?;
                Ok(parsed)
            }
            Err(err) => {
                if !self.refresh {
                    if let Some(bytes) = &cached {
                        if let Ok(value) = crate::model::parse_json(bytes, cache_name) {
                            eprintln!("warning: manifest refresh failed; using cached {cache_name}: {err:#}");
                            return Ok(value);
                        }
                    }
                }
                Err(err)
            }
        }
    }

    fn get_bytes(&self, url: &str, range: Option<u64>) -> Result<Vec<u8>> {
        let mut last = String::new();
        for attempt in 1..=3 {
            let mut req = self.agent.get(&encode_url(url));
            if let Some(offset) = range {
                req = req.set("Range", &format!("bytes={offset}-"));
            }
            match req.call() {
                Ok(resp) => {
                    let mut reader = resp.into_reader();
                    let mut out = Vec::new();
                    reader
                        .read_to_end(&mut out)
                        .with_context(|| format!("reading {url}"))?;
                    return Ok(out);
                }
                Err(err) => {
                    last = err.to_string();
                    if attempt < 3 {
                        std::thread::sleep(Duration::from_secs(attempt as u64));
                    }
                }
            }
        }
        bail!("download failed for {url}: {last}")
    }

    pub fn download_payload(&self, payload: &Payload) -> Result<PathBuf> {
        if payload.file_name.is_empty() {
            bail!("payload for {} has no fileName", payload.url);
        }
        let hash = payload.sha256.to_ascii_lowercase();
        let bucket = if hash.is_empty() { "nohash" } else { hash.as_str() };
        let dir = self.cache.join("payloads").join(bucket);
        fs::create_dir_all(&dir)?;
        let target = dir.join(safe_file_name(&payload.file_name));
        if target.exists() && !hash.is_empty() {
            if sha256_file(&target)? == hash {
                return Ok(target);
            }
            let _ = fs::remove_file(&target);
        }
        if target.exists() && hash.is_empty() {
            return Ok(target);
        }

        let part = target.with_extension("part");
        let mut last = String::new();
        for attempt in 1..=3 {
            let offset = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
            let mut req = self.agent.get(&encode_url(&payload.url));
            if offset > 0 {
                req = req.set("Range", &format!("bytes={offset}-"));
            }
            match req.call() {
                Ok(resp) => {
                    let status = resp.status();
                    let append = offset > 0 && status == 206;
                    let mut file = if append {
                        OpenOptions::new().append(true).open(&part)?
                    } else {
                        File::create(&part)?
                    };
                    let mut reader = resp.into_reader();
                    let mut buf = vec![0u8; 64 * 1024];
                    let mut total = if append { offset } else { 0 };
                    let mut last_mb = u64::MAX;
                    loop {
                        let n = reader.read(&mut buf)?;
                        if n == 0 {
                            break;
                        }
                        file.write_all(&buf[..n])?;
                        total += n as u64;
                        let mb = total / (1024 * 1024);
                        if mb != last_mb {
                            last_mb = mb;
                            print!("\r  {}  {} MB", payload.file_name, mb);
                            use std::io::Write as _;
                            let _ = std::io::stdout().flush();
                        }
                    }
                    println!();
                    drop(file);
                    if !hash.is_empty() {
                        let got = sha256_file(&part)?;
                        if got != hash {
                            let _ = fs::remove_file(&part);
                            last = format!(
                                "sha256 mismatch for {}: expected {}, got {}",
                                payload.file_name, hash, got
                            );
                            if attempt < 3 {
                                continue;
                            }
                            bail!(last);
                        }
                    }
                    fs::rename(&part, &target)?;
                    return Ok(target);
                }
                Err(err) => {
                    last = err.to_string();
                    if attempt < 3 {
                        std::thread::sleep(Duration::from_secs(attempt as u64));
                    }
                }
            }
        }
        bail!("download failed for {}: {last}", payload.file_name)
    }
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn safe_file_name(name: &str) -> String {
    let base = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .trim();
    let mut out: String = base
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    if out.is_empty() {
        out = "payload.bin".to_string();
    }
    out
}

fn encode_url(url: &str) -> String {
    // Manifest URLs are already escaped except for occasional spaces.
    url.replace(' ', "%20")
}








