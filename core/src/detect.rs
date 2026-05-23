use std::path::Path;
use walkdir::WalkDir;
use pelite::{PeFile, Wrap};
use crate::models::{BuildMode, UEVersion};

pub struct DetectResult {
    pub mode: BuildMode,
    pub version: UEVersion,
    pub evidence: String,
}

pub fn detect_ue_version(game_dir: &Path) -> Result<DetectResult, String> {
    let is_iostore = find_first_utoc(game_dir).is_some();

    // 1. 確認目錄中存在 .pak 或 .utoc，否則視為無效遊戲目錄
    if !is_iostore && find_first_pak(game_dir).is_none() {
        return Err("找不到 .pak 或 .utoc 檔案，請確認遊戲目錄是否正確。".to_string());
    }

    let mode = if is_iostore { BuildMode::IoStore } else { BuildMode::Pak };

    // 2. 從 EXE 的 VS_VERSIONINFO 讀取精確版本
    if let Some(mut result) = try_read_exe_version(game_dir) {
        result.mode = mode;
        return Ok(result);
    }

    // 3. 嘗試 Build.version 檔案
    if let Some(mut result) = try_read_build_version(game_dir) {
        result.mode = mode;
        return Ok(result);
    }

    // 找不到 Shipping.exe 且無 Build.version → 回傳錯誤
    Err("找不到 Shipping.exe，無法判斷遊戲版本。".to_string())
}

fn try_read_exe_version(game_dir: &Path) -> Option<DetectResult> {
    // 優化點：走訪中直接過濾 + 嘗試讀取，找到後立即回傳，不再先收集再迴圈
    for entry in WalkDir::new(game_dir).max_depth(5) {
        let Ok(e) = entry else { continue };
        let p = e.path();

        if !p.extension().is_some_and(|ext| ext == "exe") {
            continue;
        }
        let name = p.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
        if !p.to_string_lossy().contains("Binaries") || !name.contains("shipping") {
            continue;
        }

        if let Some((major, minor)) = scan_exe_with_pelite(p) {
            if let Some(version) = UEVersion::from_major_minor(major, minor) {
                return Some(DetectResult {
                    mode: BuildMode::Pak, // 呼叫端會覆寫
                    version,
                    evidence: format!(
                        "從 EXE ({}) 的 VS_VERSIONINFO 偵測到版本",
                        p.file_name().unwrap_or_default().to_string_lossy()
                    ),
                });
            }
        }
    }
    None
}

/// 使用 pelite 解析 PE 檔案以精準獲取版本
fn scan_exe_with_pelite(path: &Path) -> Option<(i32, i32)> {
    let bytes = std::fs::read(path).ok()?;

    macro_rules! extract_version {
        ($pe:expr) => {
            if let Ok(resources) = $pe.resources() {
                if let Ok(version_info) = resources.version_info() {
                    if let Some(fixed) = version_info.fixed() {
                        let major = fixed.dwFileVersion.Major as i32;
                        let minor = fixed.dwFileVersion.Minor as i32;
                        if major == 4 || major == 5 {
                            return Some((major, minor));
                        }
                    }
                }
            }
        };
    }

    match PeFile::from_bytes(&bytes) {
        Ok(Wrap::T64(pe)) => {
            use pelite::pe64::Pe;
            extract_version!(pe);
        }
        Ok(Wrap::T32(pe)) => {
            use pelite::pe32::Pe;
            extract_version!(pe);
        }
        _ => {}
    }
    None
}

/// 對外公開：用指定 EXE 讀取版本（供 GUI 的 WNE 流程使用）
pub fn probe_exe_version(exe_path: &Path) -> Option<UEVersion> {
    let (major, minor) = scan_exe_with_pelite(exe_path)?;
    UEVersion::from_major_minor(major, minor)
}

/// 找出所有檔名含 "WindowsNoEditor" 的 .pak 檔案
pub fn find_wne_paks(game_dir: &Path) -> Vec<std::path::PathBuf> {
    WalkDir::new(game_dir)
        .max_depth(6)
        .into_iter()
        .flatten()
        .filter(|e| {
            let p = e.path();
            p.extension().is_some_and(|x| x == "pak")
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.contains("WindowsNoEditor"))
        })
        .map(|e| e.path().to_path_buf())
        .collect()
}

/// 找出目錄內所有 .exe 檔案（含子目錄，深度 6）
pub fn find_all_exes(game_dir: &Path) -> Vec<std::path::PathBuf> {
    WalkDir::new(game_dir)
        .max_depth(6)
        .into_iter()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "exe"))
        .map(|e| e.path().to_path_buf())
        .collect()
}

fn find_first_utoc(game_dir: &Path) -> Option<std::path::PathBuf> {
    WalkDir::new(game_dir)
        .max_depth(6)
        .into_iter()
        .flatten()
        .find(|e| e.path().extension().is_some_and(|x| x == "utoc"))
        .map(|e| e.path().to_path_buf())
}

fn find_first_pak(game_dir: &Path) -> Option<std::path::PathBuf> {
    WalkDir::new(game_dir)
        .max_depth(6)
        .into_iter()
        .flatten()
        .find(|e| e.path().extension().is_some_and(|x| x == "pak"))
        .map(|e| e.path().to_path_buf())
}

fn try_read_build_version(game_dir: &Path) -> Option<DetectResult> {
    for candidate in &["Engine/Build/Build.version", "Build/Build.version"] {
        let path = game_dir.join(candidate);
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Some(result) = parse_build_version(&content) {
                return Some(result);
            }
        }
    }
    None
}

fn parse_build_version(content: &str) -> Option<DetectResult> {
    let major = extract_json_int(content, "MajorVersion")?;
    let minor = extract_json_int(content, "MinorVersion")?;
    let version = UEVersion::from_major_minor(major, minor)?;
    Some(DetectResult {
        mode: BuildMode::Pak, // 呼叫端會覆寫
        version,
        evidence: format!("Build.version: {}.{}", major, minor),
    })
}

fn extract_json_int(content: &str, key: &str) -> Option<i32> {
    let pattern = format!("\"{}\"", key);
    let pos = content.find(&pattern)?;
    let rest = &content[pos + pattern.len()..];
    let colon_pos = rest.find(':')?;
    let value_str = rest[colon_pos + 1..].trim_start();
    let end = value_str
        .find(|c: char| !c.is_ascii_digit() && c != '-')
        .unwrap_or(value_str.len());
    value_str[..end].trim().parse().ok()
}