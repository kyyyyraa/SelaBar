use crate::backend::system;
use serde::Deserialize;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const REPO_URL: &str = "https://github.com/kyyyyraa/SelaBar";
pub const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/kyyyyraa/SelaBar/releases/latest";

const MAX_UPDATE_BYTES: u64 = 150 * 1024 * 1024;
const MIN_EXE_BYTES: u64 = 200_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateReleaseInfo {
    pub version: String,
    pub tag_name: String,
    pub html_url: String,
    pub notes: String,
    pub asset_name: String,
    pub asset_url: String,
    pub asset_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheckOutcome {
    UpToDate { current: String, latest: String },
    UpdateAvailable(UpdateReleaseInfo),
    Offline,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAssetCandidate {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
}

#[derive(Debug, Deserialize)]
struct GithubAssetJson {
    #[serde(default)]
    name: String,
    #[serde(default)]
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

#[derive(Debug, Deserialize)]
struct GithubReleaseJson {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GithubAssetJson>,
}

/// Parses semver tags like `"v0.2.1"`, `"0.2.0"`, or `"0.2.1-beta.1"` into `(major, minor, patch)`.
pub fn parse_semver(raw: &str) -> Option<(u64, u64, u64)> {
    let trimmed = raw.trim().trim_start_matches(['v', 'V']);
    if trimmed.is_empty() {
        return None;
    }
    let core = trimmed
        .split(['-', '+'])
        .next()
        .unwrap_or("")
        .trim();
    let mut parts = core.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = match parts.next() {
        Some(p) => p.parse::<u64>().ok()?,
        None => 0,
    };
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

pub fn normalize_version_display(raw: &str) -> String {
    match parse_semver(raw) {
        Some((maj, min, pat)) => format!("{}.{}.{}", maj, min, pat),
        None => raw.trim().trim_start_matches(['v', 'V']).to_string(),
    }
}

pub fn is_newer_version(current: &str, candidate: &str) -> bool {
    matches!(
        (parse_semver(current), parse_semver(candidate)),
        (Some(cur), Some(cand)) if cand > cur
    )
}

/// Allows strictly HTTPS GitHub hosts without userinfo (`@`), whitespace, or backslashes.
pub fn is_allowed_github_url(url: &str) -> bool {
    let trimmed = url.trim();
    let rest = match trimmed.strip_prefix("https://") {
        Some(r) => r,
        None => return false,
    };
    if rest.is_empty()
        || rest.contains(['@', '\\', ' ', '\r', '\n', '\t'])
    {
        return false;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_only = authority
        .strip_suffix(":443")
        .unwrap_or(authority)
        .to_ascii_lowercase();
    if host_only.is_empty() {
        return false;
    }
    host_only == "github.com"
        || host_only == "api.github.com"
        || (host_only.ends_with(".githubusercontent.com")
            && host_only.len() > ".githubusercontent.com".len()
            && !host_only.starts_with('.')
            && !host_only.contains(".."))
}

pub fn sanitize_asset_filename(name: &str) -> Option<String> {
    let t = name.trim();
    if t.is_empty()
        || t.len() > 160
        || t == "."
        || t == ".."
        || t.contains(['/', '\\', ':', '\0'])
        || t.contains("..")
    {
        return None;
    }
    if t.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        Some(t.to_string())
    } else {
        None
    }
}

pub fn select_best_asset(assets: &[ReleaseAssetCandidate]) -> Option<(String, String, u64)> {
    let mut best: Option<(u8, String, String, u64)> = None;
    for a in assets {
        let Some(clean_name) = sanitize_asset_filename(&a.name) else {
            continue;
        };
        if !is_allowed_github_url(&a.browser_download_url) {
            continue;
        }
        let low = clean_name.to_ascii_lowercase();
        let priority = if low.ends_with("-setup.exe") || (low.contains("setup") && low.ends_with(".exe")) {
            3
        } else if low == "selabar.exe" || low.ends_with(".exe") {
            2
        } else if low.ends_with(".zip") {
            1
        } else {
            0
        };
        if priority == 0 {
            continue;
        }
        if best.as_ref().map_or(true, |(bp, _, _, _)| priority > *bp) {
            best = Some((priority, clean_name, a.browser_download_url.clone(), a.size));
        }
    }
    best.map(|(_, n, u, s)| (n, u, s))
}

pub async fn check_latest_release() -> UpdateCheckOutcome {
    if system::internet_connected() == Some(false) {
        return UpdateCheckOutcome::Offline;
    }

    let client = match reqwest::Client::builder()
        .user_agent(format!("SelaBar/{}", CURRENT_VERSION))
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(12))
        .build()
    {
        Ok(c) => c,
        Err(e) => return UpdateCheckOutcome::Error(format!("HTTP client error: {}", e)),
    };

    let resp = match client
        .get(LATEST_RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            if e.is_timeout() {
                return UpdateCheckOutcome::Error("Waktu cek update habis (timeout).".to_string());
            }
            if e.is_connect() {
                return UpdateCheckOutcome::Offline;
            }
            return UpdateCheckOutcome::Error(format!("Gagal menghubungi GitHub: {}", e));
        }
    };

    let status = resp.status();
    if status.as_u16() == 404 {
        return UpdateCheckOutcome::UpToDate {
            current: CURRENT_VERSION.to_string(),
            latest: CURRENT_VERSION.to_string(),
        };
    }
    if status.as_u16() == 403 || status.as_u16() == 429 {
        return UpdateCheckOutcome::Error(
            "Batas request GitHub API tercapai; coba lagi beberapa menit.".to_string(),
        );
    }
    if !status.is_success() {
        return UpdateCheckOutcome::Error(format!("GitHub API HTTP {}", status.as_u16()));
    }

    let rel: GithubReleaseJson = match resp.json().await {
        Ok(j) => j,
        Err(e) => return UpdateCheckOutcome::Error(format!("Format rilis tidak valid: {}", e)),
    };

    if rel.draft || rel.prerelease || rel.tag_name.trim().is_empty() {
        return UpdateCheckOutcome::UpToDate {
            current: CURRENT_VERSION.to_string(),
            latest: CURRENT_VERSION.to_string(),
        };
    }

    let latest_ver = normalize_version_display(&rel.tag_name);
    if !is_newer_version(CURRENT_VERSION, &rel.tag_name) {
        return UpdateCheckOutcome::UpToDate {
            current: CURRENT_VERSION.to_string(),
            latest: latest_ver,
        };
    }

    let candidates: Vec<ReleaseAssetCandidate> = rel
        .assets
        .into_iter()
        .map(|a| ReleaseAssetCandidate {
            name: a.name,
            browser_download_url: a.browser_download_url,
            size: a.size,
        })
        .collect();
    let (asset_name, asset_url, asset_size) = select_best_asset(&candidates).unwrap_or_default();
    let html_url = if is_allowed_github_url(&rel.html_url) {
        rel.html_url
    } else {
        format!("{}/releases/latest", REPO_URL)
    };
    let notes = rel.body.trim().chars().take(480).collect::<String>();

    UpdateCheckOutcome::UpdateAvailable(UpdateReleaseInfo {
        version: latest_ver,
        tag_name: rel.tag_name,
        html_url,
        notes,
        asset_name,
        asset_url,
        asset_size,
    })
}

pub async fn download_and_launch_installer(info: &UpdateReleaseInfo) -> Result<String, String> {
    let fname = sanitize_asset_filename(&info.asset_name);
    let is_exe = fname
        .as_deref()
        .is_some_and(|n| n.to_ascii_lowercase().ends_with(".exe"));

    if info.asset_url.is_empty() || !is_exe {
        let target = if is_allowed_github_url(&info.html_url) {
            info.html_url.clone()
        } else {
            format!("{}/releases/latest", REPO_URL)
        };
        system::open_url_async(&target);
        return Ok(format!(
            "Halaman rilis v{} dibuka di browser.",
            info.version
        ));
    }

    if !is_allowed_github_url(&info.asset_url) {
        return Err("URL unduhan update ditolak (bukan domain GitHub resmi).".to_string());
    }

    let clean_name = fname.ok_or_else(|| "Nama file installer tidak valid.".to_string())?;
    let update_dir: PathBuf = std::env::temp_dir().join("SelaBar-Update");
    tokio::fs::create_dir_all(&update_dir)
        .await
        .map_err(|e| format!("Gagal membuat folder sementara update: {}", e))?;

    let part_path = update_dir.join(format!("{}.part", clean_name));
    let final_path = update_dir.join(&clean_name);
    let _ = tokio::fs::remove_file(&part_path).await;

    let client = reqwest::Client::builder()
        .user_agent(format!("SelaBar/{}", CURRENT_VERSION))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let mut resp = client
        .get(&info.asset_url)
        .send()
        .await
        .map_err(|e| format!("Gagal mengunduh update: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("Gagal mengunduh update: HTTP {}", resp.status().as_u16()));
    }
    if let Some(len) = resp.content_length() {
        if len > MAX_UPDATE_BYTES {
            return Err("Ukuran file update melebihi batas aman 150 MB.".to_string());
        }
    }

    let mut file = tokio::fs::File::create(&part_path)
        .await
        .map_err(|e| format!("Gagal menyimpan installer: {}", e))?;

    let mut total: u64 = 0;
    let mut header = [0u8; 2];
    let mut header_len = 0usize;

    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("Stream unduhan terputus: {}", e))?
    {
        total = total.saturating_add(chunk.len() as u64);
        if total > MAX_UPDATE_BYTES {
            drop(file);
            let _ = tokio::fs::remove_file(&part_path).await;
            return Err("File unduhan melebihi batas 150 MB.".to_string());
        }
        for &b in chunk.iter().take(2 - header_len) {
            header[header_len] = b;
            header_len += 1;
        }
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("Gagal menulis file update: {}", e))?;
    }
    file.flush()
        .await
        .map_err(|e| format!("Gagal menyelesaikan file update: {}", e))?;
    drop(file);

    if header_len < 2 || &header != b"MZ" || total < MIN_EXE_BYTES {
        let _ = tokio::fs::remove_file(&part_path).await;
        return Err("File installer tidak valid atau rusak.".to_string());
    }

    let _ = tokio::fs::remove_file(&final_path).await;
    tokio::fs::rename(&part_path, &final_path)
        .await
        .map_err(|e| format!("Gagal menyiapkan installer: {}", e))?;

    std::process::Command::new(&final_path)
        .spawn()
        .map_err(|e| format!("Gagal menjalankan installer: {}", e))?;

    Ok(format!(
        "Installer v{} ({}) dijalankan.",
        info.version, clean_name
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_parsing_and_comparison() {
        assert_eq!(parse_semver("0.1.0"), Some((0, 1, 0)));
        assert_eq!(parse_semver("v0.2.3"), Some((0, 2, 3)));
        assert_eq!(parse_semver("V1.4"), Some((1, 4, 0)));
        assert_eq!(parse_semver("1.2.3-rc.1"), Some((1, 2, 3)));
        assert_eq!(parse_semver("invalid"), None);
        assert_eq!(parse_semver("1.2.3.4"), None);

        assert!(is_newer_version("0.1.0", "v0.1.1"));
        assert!(is_newer_version("0.1.0", "0.2.0"));
        assert!(is_newer_version("0.9.9", "1.0.0"));
        assert!(!is_newer_version("0.1.0", "v0.1.0"));
        assert!(!is_newer_version("0.2.0", "v0.1.9"));
        assert!(!is_newer_version("0.1.0", "not-a-version"));
    }

    #[test]
    fn github_url_allowlist_blocks_spoofing_and_non_https() {
        assert!(is_allowed_github_url("https://github.com/kyyyyraa/SelaBar/releases/latest"));
        assert!(is_allowed_github_url("https://api.github.com/repos/kyyyyraa/SelaBar/releases/latest"));
        assert!(is_allowed_github_url("https://objects.githubusercontent.com/github-production-release-asset/123/SelaBar-Setup.exe"));
        assert!(is_allowed_github_url("https://release-assets.githubusercontent.com/abc.exe"));

        assert!(!is_allowed_github_url("http://github.com/kyyyyraa/SelaBar"));
        assert!(!is_allowed_github_url("https://github.com@evil.com/payload.exe"));
        assert!(!is_allowed_github_url("https://evilgithub.com/payload.exe"));
        assert!(!is_allowed_github_url("https://.githubusercontent.com/x.exe"));
        assert!(!is_allowed_github_url("https://github.com\\..\\evil.exe"));
        assert!(!is_allowed_github_url("javascript:alert(1)"));
    }

    #[test]
    fn asset_filename_sanitization_and_priority_selection() {
        assert_eq!(
            sanitize_asset_filename("SelaBar-v0.2.0-Setup.exe"),
            Some("SelaBar-v0.2.0-Setup.exe".to_string())
        );
        assert_eq!(sanitize_asset_filename("..\\SelaBar.exe"), None);
        assert_eq!(sanitize_asset_filename("dir/SelaBar.exe"), None);
        assert_eq!(sanitize_asset_filename("C:SelaBar.exe"), None);
        assert_eq!(sanitize_asset_filename("bad name.exe"), None);

        let assets = vec![
            ReleaseAssetCandidate {
                name: "SelaBar-v0.2.0-win64-portable.zip".to_string(),
                browser_download_url: "https://github.com/kyyyyraa/SelaBar/releases/download/v0.2.0/SelaBar-v0.2.0-win64-portable.zip".to_string(),
                size: 5_500_000,
            },
            ReleaseAssetCandidate {
                name: "SelaBar-v0.2.0-Setup.exe".to_string(),
                browser_download_url: "https://github.com/kyyyyraa/SelaBar/releases/download/v0.2.0/SelaBar-v0.2.0-Setup.exe".to_string(),
                size: 7_500_000,
            },
            ReleaseAssetCandidate {
                name: "SelaBar.exe".to_string(),
                browser_download_url: "https://github.com/kyyyyraa/SelaBar/releases/download/v0.2.0/SelaBar.exe".to_string(),
                size: 13_500_000,
            },
        ];
        let best = select_best_asset(&assets).unwrap();
        assert_eq!(best.0, "SelaBar-v0.2.0-Setup.exe");

        let blocked = vec![ReleaseAssetCandidate {
            name: "SelaBar-v0.2.0-Setup.exe".to_string(),
            browser_download_url: "https://evil.example.com/Setup.exe".to_string(),
            size: 1000,
        }];
        assert!(select_best_asset(&blocked).is_none());
    }
}
