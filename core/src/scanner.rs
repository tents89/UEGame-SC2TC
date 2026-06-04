use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use rayon::prelude::*;

use crate::models::{PakEntry, TreeNode};

// ── PAK / UTOC 掃描 ───────────────────────────────────────────────────────────

pub fn find_all_paks(game_dir: &Path) -> Vec<PathBuf> {
    let mut paks: Vec<PathBuf> = WalkDir::new(game_dir)
        .max_depth(6)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let ext = path.extension()?.to_str()?;
            if !ext.eq_ignore_ascii_case("pak") && !ext.eq_ignore_ascii_case("utoc") {
                return None;
            }

            // 優化點：統一轉小寫後進行一次性判斷，比原本的四條件判斷更清晰
            let path_lower = path.to_string_lossy().to_ascii_lowercase();
            if path_lower.contains("/engine/") || path_lower.contains("\\engine\\") {
                return None;
            }

            Some(path.to_path_buf())
        })
        .collect();

    paks.sort_unstable();
    paks
}

pub fn scan_pak(container_path: &Path, aes_key_hex: Option<&str>) -> Result<Vec<PakEntry>> {
    scan_pak_with_options(container_path, aes_key_hex, false)
}

pub fn scan_pak_all(container_path: &Path, aes_key_hex: Option<&str>) -> Result<Vec<PakEntry>> {
    scan_pak_with_options(container_path, aes_key_hex, true)
}

fn scan_pak_with_options(
    container_path: &Path,
    aes_key_hex: Option<&str>,
    all_files: bool,
) -> Result<Vec<PakEntry>> {
    let ext = container_path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();

    if ext == "utoc" {
        scan_pak_utoc(container_path, aes_key_hex, all_files)
    } else {
        scan_pak_pak(container_path, aes_key_hex, all_files)
    }
}

fn is_target_file(lower_path: &str) -> bool {
    lower_path.ends_with(".locres")
        || lower_path.ends_with(".ufont")
        || lower_path.ends_with(".ttf")
        || lower_path.ends_with(".otf")
}

fn is_engine_path(lower_path: &str) -> bool {
    // PAK 內部路徑統一使用 '/'。需精準比對「engine」目錄而非單純子字串，
    // 否則 `engineering/` 等路徑會被誤判排除。
    lower_path.starts_with("engine/") || lower_path.contains("/engine/")
}

fn scan_pak_utoc(container_path: &Path, aes_key_hex: Option<&str>, all_files: bool) -> Result<Vec<PakEntry>> {
    let mut config = retoc::Config::default();
    if let Some(key_hex) = aes_key_hex {
        let k = std::str::FromStr::from_str(key_hex)
            .map_err(|_| anyhow::anyhow!("AES Key 格式錯誤 (IoStore): {}", key_hex))?;
        config.aes_keys.insert(retoc::FGuid::default(), k);
    }

    let store = retoc::iostore::open(container_path, std::sync::Arc::new(config))
        .with_context(|| format!("無法打開 IoStore: {}", container_path.display()))?;

    let result = store
        .chunks()
        .filter_map(|chunk| {
            let p = chunk.path()?;
            let clean_path = p.strip_prefix("../../../").unwrap_or(&p).to_string();
            let line_lower = clean_path.to_lowercase();

            if is_engine_path(&line_lower) {
                return None;
            }
            if !all_files && !is_target_file(&line_lower) {
                return None;
            }

            Some(PakEntry {
                pak: container_path.to_path_buf(),
                path: clean_path,
                size: Some(chunk.size()),
            })
        })
        .collect();

    Ok(result)
}

fn scan_pak_pak(container_path: &Path, aes_key_hex: Option<&str>, all_files: bool) -> Result<Vec<PakEntry>> {
    use std::fs::File;
    use std::io::BufReader;
    let mut file = BufReader::new(File::open(container_path)?);

    let mut builder = repak::PakBuilder::new();
    if let Some(key_hex) = aes_key_hex {
        let clean_hex = key_hex.trim_start_matches("0x");
        let key_bytes = hex::decode(clean_hex)
            .map_err(|e| anyhow::anyhow!("AES Key hex 解碼失敗: {}", e))?;
        use aes::cipher::KeyInit;
        let aes_key = aes::Aes256::new_from_slice(&key_bytes)
            .map_err(|_| anyhow::anyhow!("AES Key 長度錯誤 (需 32 bytes / 64 hex 字元)"))?;
        builder = builder.key(aes_key);
    }

    let pak_reader = builder
        .reader(&mut file)
        .with_context(|| format!("無法建立 PakReader: {}", container_path.display()))?;

    let mount_point = pak_reader.mount_point().to_string();

    let result = pak_reader
        .files()
        .into_iter()
        .filter_map(|path| {
            let full_mounted = format!("{}{}", mount_point, path);
            let clean_path = full_mounted
                .strip_prefix("../../../")
                .unwrap_or(&full_mounted)
                .to_string();
            let line_lower = clean_path.to_lowercase();

            if is_engine_path(&line_lower) {
                return None;
            }
            if !all_files && !is_target_file(&line_lower) {
                return None;
            }

            Some(PakEntry {
                pak: container_path.to_path_buf(),
                path: clean_path,
                size: None,
            })
        })
        .collect();

    Ok(result)
}

pub fn scan_all_paks(paks: &[PathBuf], aes_key: Option<&str>) -> Vec<(PathBuf, Result<Vec<PakEntry>>)> {
    paks.par_iter()
        .map(|pak| (pak.clone(), scan_pak(pak, aes_key)))
        .collect()
}

pub fn scan_all_paks_full(paks: &[PathBuf], aes_key: Option<&str>) -> Vec<(PathBuf, Result<Vec<PakEntry>>)> {
    paks.par_iter()
        .map(|pak| (pak.clone(), scan_pak_all(pak, aes_key)))
        .collect()
}

// ── 樹狀結構建立 ─────────────────────────────────────────────────────────────

pub fn build_tree(entries: &[PakEntry]) -> Vec<TreeNode> {
    let mut root: Vec<TreeNode> = vec![];
    for entry in entries {
        let pak_name = entry
            .pak
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let mut parts: Vec<&str> = vec![&pak_name];
        parts.extend(entry.path.split('/'));
        insert_path(&mut root, &parts, entry);
    }
    sort_tree(&mut root);
    root
}

fn insert_path(nodes: &mut Vec<TreeNode>, parts: &[&str], entry: &PakEntry) {
    if parts.len() == 1 {
        nodes.push(TreeNode::File(entry.clone()));
        return;
    }
    let dir_name = parts[0];

    // 優化點：用 position + 直接存取，避免巢狀 if let
    if let Some(idx) = nodes.iter().position(|n| matches!(n, TreeNode::Dir { name, .. } if name == dir_name)) {
        if let TreeNode::Dir { children, .. } = &mut nodes[idx] {
            insert_path(children, &parts[1..], entry);
        }
    } else {
        let mut children = vec![];
        insert_path(&mut children, &parts[1..], entry);
        nodes.push(TreeNode::Dir {
            name: dir_name.to_string(),
            children,
            expanded: true,
        });
    }
}

fn sort_tree(nodes: &mut Vec<TreeNode>) {
    nodes.sort_by(|a, b| match (a.is_dir(), b.is_dir()) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name().cmp(b.name()),
    });
    for node in nodes.iter_mut() {
        if let TreeNode::Dir { children, .. } = node {
            sort_tree(children);
        }
    }
}

pub fn count_entries(nodes: &[TreeNode]) -> (usize, usize) {
    nodes.iter().fold((0, 0), |(dirs, files), node| match node {
        TreeNode::Dir { children, .. } => {
            let (d, f) = count_entries(children);
            (dirs + 1 + d, files + f)
        }
        TreeNode::File(_) => (dirs, files + 1),
    })
}