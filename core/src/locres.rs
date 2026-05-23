use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::models::LocresEntry;
use locres_rs::locres::{self as locres_api};

// ── 容器讀取 ──────────────────────────────────────────────────────────────────

pub fn read_raw_file_from_container(
    container: &Path,
    internal_path: &str,
    aes_key_hex: Option<&str>,
) -> Result<Vec<u8>> {
    let ext = container
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();

    if ext == "utoc" {
        let mut config = retoc::Config::default();
        if let Some(key_hex) = aes_key_hex {
            if let Ok(k) = std::str::FromStr::from_str(key_hex) {
                config.aes_keys.insert(retoc::FGuid::default(), k);
            }
        }
        let store = retoc::iostore::open(container, std::sync::Arc::new(config))
            .with_context(|| format!("無法打開 IoStore: {}", container.display()))?;

        for chunk in store.chunks() {
            if let Some(p) = chunk.path() {
                let clean_path = p.strip_prefix("../../../").unwrap_or(&p);
                if clean_path == internal_path {
                    return chunk.read().context("無法讀取 Chunk 數據");
                }
            }
        }
        anyhow::bail!("在 IoStore 找不到目標檔案: {}", internal_path);
    } else {
        use std::fs::File;
        use std::io::BufReader;
        let mut file = BufReader::new(File::open(container)?);

        let mut builder = repak::PakBuilder::new();
        if let Some(key_hex) = aes_key_hex {
            let clean_hex = key_hex.trim_start_matches("0x");
            if let Ok(key_bytes) = hex::decode(clean_hex) {
                use aes::cipher::KeyInit;
                if let Ok(aes_key) = aes::Aes256::new_from_slice(&key_bytes) {
                    builder = builder.key(aes_key);
                }
            }
        }

        let pak_reader = builder
            .reader(&mut file)
            .with_context(|| "無法建立 PakReader")?;

        let mount_point = pak_reader.mount_point().to_string();
        let target_path = pak_reader.files().into_iter().find(|p| {
            let full = format!("{}{}", mount_point, p);
            let clean = full.strip_prefix("../../../").unwrap_or(&full);
            clean == internal_path
        });

        let target_path = target_path.ok_or_else(|| {
            anyhow::anyhow!("在 pak 中找不到對應的掛載路徑: {}", internal_path)
        })?;

        pak_reader
            .get(&target_path, &mut file)
            .with_context(|| format!("無法從 pak 讀取: {}", internal_path))
    }
}

// ── Locres 讀取 ───────────────────────────────────────────────────────────────

pub fn read_locres_from_pak(
    container: &Path,
    internal_path: &str,
    aes_key_hex: Option<&str>,
) -> Result<Vec<LocresEntry>> {
    let data = read_raw_file_from_container(container, internal_path, aes_key_hex)?;

    // 優化點：使用 SystemTime 毫秒 + unwrap_or_default 避免 panic
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let temp_path = std::env::temp_dir().join(format!("ue_mod_locres_{}.tmp", ts));

    std::fs::write(&temp_path, &data)?;
    let result = parse_locres(&temp_path);
    let _ = std::fs::remove_file(&temp_path);
    result
}

pub fn extract_locres_to_file(
    container: &Path,
    internal_path: &str,
    output_path: &Path,
    aes_key_hex: Option<&str>,
) -> Result<()> {
    let data = read_raw_file_from_container(container, internal_path, aes_key_hex)?;
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output_path, &data)?;
    Ok(())
}

pub fn parse_locres(path: &Path) -> Result<Vec<LocresEntry>> {
    let path_str = path.to_string_lossy();
    let locres_res = locres_api::read(path_str.as_ref())
        .map_err(|e| anyhow::anyhow!("Locres read error: {}", e))
        .with_context(|| format!("無法解析 locres: {}", path.display()))?;

    let entries = locres_res
        .namespaces
        .iter()
        .flat_map(|(ns_name, ns_obj)| {
            ns_obj.entries.values().map(move |entry_obj| LocresEntry {
                namespace: ns_name.clone(),
                key: entry_obj.key.clone(),
                value: entry_obj.translation.replace('\r', ""),
                modified: None,
            })
        })
        .collect();

    Ok(entries)
}

// ── CSV 匯出 / 匯入 ───────────────────────────────────────────────────────────

pub fn export_to_csv(entries: &[LocresEntry], output: &Path) -> Result<()> {
    let file = fs::File::create(output)
        .with_context(|| format!("無法建立 CSV: {}", output.display()))?;
    let mut writer = BufWriter::new(file);

    // UTF-8 BOM，確保 Excel 正確開啟
    writer.write_all(&[0xEF, 0xBB, 0xBF])?;
    writeln!(writer, "namespace,key,value")?;

    let mut csv_writer = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(writer);
    for entry in entries {
        csv_writer.write_record(&[&entry.namespace, &entry.key, entry.effective_value()])?;
    }
    csv_writer.flush()?;
    Ok(())
}

pub fn import_from_csv(path: &Path, current_entries: &mut Vec<LocresEntry>) -> Result<usize> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_path(path)?;

    let mut csv_map: HashMap<String, String> = HashMap::new();
    for record in reader.records() {
        let record = record?;
        if record.len() < 3 {
            continue;
        }
        let composite_key = format!("{}\0{}", &record[0], &record[1]);
        let new_value = record[2].replace('\r', "");
        csv_map.insert(composite_key, new_value);
    }

    let mut modified_count = 0;
    for entry in current_entries.iter_mut() {
        // 用 format! 組合鍵進行查找，避免 tuple lookup 的雙重 clone
        let lookup = format!("{}\0{}", entry.namespace, entry.key);
        if let Some(new_val) = csv_map.get(&lookup) {
            if new_val != &entry.value {
                entry.modified = Some(new_val.clone());
                modified_count += 1;
            } else {
                entry.modified = None;
            }
        }
    }

    Ok(modified_count)
}

// ── 簡轉繁 ───────────────────────────────────────────────────────────────────

pub fn convert_to_traditional(entries: &mut Vec<LocresEntry>, target_mode: &str) -> Result<()> {
    let converter = opencc_rust::converter("cn", target_mode)
        .map_err(|e| anyhow::anyhow!("OpenCC error: {}", e))?;

    for entry in entries.iter_mut() {
        let converted = converter.convert(&entry.value);
        if converted != entry.value {
            entry.modified = Some(converted);
        }
    }
    Ok(())
}

// ── Locres 寫入 ───────────────────────────────────────────────────────────────

pub fn write_locres(entries: &[LocresEntry], template_path: &Path, output_path: &Path) -> Result<()> {
    let template_path_str = template_path.to_string_lossy();
    let mut locres_res = locres_api::read(template_path_str.as_ref())
        .map_err(|e| anyhow::anyhow!("Locres read error: {}", e))?;

    // 優化點：原版是 O(entries × namespaces × ns_entries) 的三層巢狀迴圈。
    // 現在先建立 HashMap<(namespace, key) -> new_value>，
    // 再對 locres_res 單次走訪，複雜度降為 O(entries + namespaces × ns_entries)。
    let modifications: HashMap<(&str, &str), &str> = entries
        .iter()
        .filter(|e| e.is_modified())
        .map(|e| ((e.namespace.as_str(), e.key.as_str()), e.effective_value()))
        .collect();

    if !modifications.is_empty() {
        for (ns_name, ns_obj) in &mut locres_res.namespaces {
            for e_obj in ns_obj.entries.values_mut() {
                if let Some(&new_val) = modifications.get(&(ns_name.as_str(), e_obj.key.as_str())) {
                    e_obj.translation = new_val.to_string();
                }
            }
        }
    }

    let output_path_str = output_path.to_string_lossy();
    locres_api::write(output_path_str.as_ref(), &locres_res)
        .map_err(|e| anyhow::anyhow!("Locres write error: {}", e))?;

    Ok(())
}