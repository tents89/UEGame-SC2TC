// 開發者模式批量導出：
//
// 對 .pak：採用原本「讀取 chunk → 直接落地」的策略。
// 對 .utoc：當目標檔案是 .uasset 時，呼叫 retoc 的 build_legacy 一併導出
//          .uasset / .uexp / .ubulk / .uptnl 等 Legacy 資產檔，避免新版 IoStore
//          只能取到 Zen 格式 .uasset、無法被 UAssetGUI 之類工具讀取的問題。
//
// 為了避免對每個檔案重開 IoStore（重 cost），這個模組以「依容器分組」為粒度，
// 一次性 open / build context / 處理所有相關內部路徑。

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use retoc::asset_conversion::{self, FZenPackageContext};
use retoc::iostore::{self, IoStoreTrait};
use retoc::logging::Log;
use retoc::{Config, EIoChunkType, FGuid, FIoChunkId, FPackageId, FileWriterTrait, UEPath};

/// build_legacy 寫檔時收集寫出檔名，供呼叫端做統計與回報。
struct RecordingFsWriter {
    dir: std::path::PathBuf,
    written: Mutex<Vec<String>>,
}

impl RecordingFsWriter {
    fn new(dir: std::path::PathBuf) -> Self {
        Self { dir, written: Mutex::new(Vec::new()) }
    }
    fn into_written(self) -> Vec<String> {
        self.written.into_inner().unwrap_or_default()
    }
}

impl FileWriterTrait for RecordingFsWriter {
    fn write_file(&self, path: String, _allow_compress: bool, data: Vec<u8>) -> Result<()> {
        let full = self.dir.join(&path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("無法建立目錄 {}", parent.display()))?;
        }
        std::fs::write(&full, &data)
            .with_context(|| format!("無法寫入 {}", full.display()))?;
        self.written.lock().unwrap().push(path);
        Ok(())
    }
}

/// 將容器內指定的 internal_paths 導出到 dest_dir。
///
/// 回傳 (internal_path, 結果) 對照，結果成功時包含實際寫出的相對檔案清單
/// （例如 `.uasset` 之後可能會附帶 `.uexp / .ubulk` 等）。
pub fn export_entries_to_dir(
    container: &Path,
    internal_paths: &[String],
    dest_dir: &Path,
    aes_key_hex: Option<&str>,
) -> Vec<(String, Result<Vec<String>>)> {
    let ext = container
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();

    if ext == "utoc" {
        export_entries_utoc(container, internal_paths, dest_dir, aes_key_hex)
    } else {
        export_entries_pak(container, internal_paths, dest_dir, aes_key_hex)
    }
}

fn write_raw_to_dest(internal_path: &str, dest_dir: &Path, bytes: &[u8]) -> Result<Vec<String>> {
    let rel = internal_path.replace('\\', "/");
    let dest = dest_dir.join(&rel);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("無法建立目錄 {}", parent.display()))?;
    }
    std::fs::write(&dest, bytes)
        .with_context(|| format!("無法寫入 {}", dest.display()))?;
    Ok(vec![rel])
}

fn export_entries_pak(
    container: &Path,
    internal_paths: &[String],
    dest_dir: &Path,
    aes_key_hex: Option<&str>,
) -> Vec<(String, Result<Vec<String>>)> {
    internal_paths
        .iter()
        .map(|p| {
            let result = (|| -> Result<Vec<String>> {
                let bytes = crate::locres::read_raw_file_from_container(
                    container, p, aes_key_hex,
                )?;
                write_raw_to_dest(p, dest_dir, &bytes)
            })();
            (p.clone(), result)
        })
        .collect()
}

fn build_config(aes_key_hex: Option<&str>) -> Result<Config> {
    let mut config = Config::default();
    if let Some(key_hex) = aes_key_hex {
        let k = std::str::FromStr::from_str(key_hex)
            .map_err(|_| anyhow::anyhow!("AES Key 格式錯誤 (IoStore): {}", key_hex))?;
        config.aes_keys.insert(FGuid::default(), k);
    }
    Ok(config)
}

/// 嘗試以「整個資料夾」開啟 IoStore（會把 global.utoc 一併納入，是 build_legacy
/// 取 ScriptObjects 的必要條件）。失敗時退回單一 .utoc 開啟，至少還能做 raw chunk 讀取。
fn open_iostore_with_globals(
    container: &Path,
    aes_key_hex: Option<&str>,
) -> Result<(Box<dyn IoStoreTrait>, bool /* opened_with_globals */)> {
    let parent = container.parent();
    if let Some(dir) = parent {
        if dir.is_dir() {
            let cfg = build_config(aes_key_hex)?;
            match iostore::open(dir, Arc::new(cfg)) {
                Ok(s) => return Ok((s, true)),
                Err(_) => {
                    // 例如不同版本 .utoc 混在同層導致 IoStoreBackend 拒絕 → 退回單檔。
                }
            }
        }
    }
    let cfg = build_config(aes_key_hex)?;
    let s = iostore::open(container, Arc::new(cfg))
        .with_context(|| format!("無法打開 IoStore: {}", container.display()))?;
    Ok((s, false))
}

fn export_entries_utoc(
    container: &Path,
    internal_paths: &[String],
    dest_dir: &Path,
    aes_key_hex: Option<&str>,
) -> Vec<(String, Result<Vec<String>>)> {
    // 開店失敗就把所有 internal_path 都標為失敗，呼叫端會看到錯誤訊息。
    let (store, _has_globals) = match open_iostore_with_globals(container, aes_key_hex) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("{:#}", e);
            return internal_paths
                .iter()
                .map(|p| (p.clone(), Err(anyhow::anyhow!(msg.clone()))))
                .collect();
        }
    };

    // clean_path -> package_id 對照（只給 .uasset 走 build_legacy 用）。
    let mut path_to_pid: HashMap<String, FPackageId> = HashMap::new();
    for pkg in store.packages() {
        let cid = FIoChunkId::from_package_id(pkg.id(), 0, EIoChunkType::ExportBundleData);
        if let Some(p) = store.chunk_path(cid) {
            let clean = p.strip_prefix("../../../").unwrap_or(&p).to_string();
            path_to_pid.insert(clean, pkg.id());
        }
    }

    // build_legacy 需要的 context；同一容器內所有資產共用，避免重複 lookup。
    let log = Log::no_log();
    let context = FZenPackageContext::create(&*store, None, &log, None);

    let mut results: Vec<(String, Result<Vec<String>>)> = Vec::with_capacity(internal_paths.len());
    for ip in internal_paths {
        let lower = ip.to_ascii_lowercase();
        let is_uasset = lower.ends_with(".uasset");

        let mut handled = false;
        if is_uasset {
            if let Some(&pid) = path_to_pid.get(ip) {
                let writer = RecordingFsWriter::new(dest_dir.to_path_buf());
                let build_res = asset_conversion::build_legacy(
                    &context,
                    pid,
                    UEPath::new(ip),
                    &writer,
                )
                .with_context(|| format!("build_legacy 失敗: {}", ip));

                match build_res {
                    Ok(()) => {
                        results.push((ip.clone(), Ok(writer.into_written())));
                        handled = true;
                    }
                    Err(_e) => {
                        // build_legacy 失敗（例如缺 global.utoc 的 ScriptObjects）：
                        // 退回 raw chunk 讀寫，至少還能拿到原始 .uasset bytes。
                    }
                }
            }
        }

        if !handled {
            // 非 .uasset、找不到對應 package、或 build_legacy 失敗：原樣讀寫 chunk。
            let result = (|| -> Result<Vec<String>> {
                let bytes = crate::locres::read_raw_file_from_container(
                    container, ip, aes_key_hex,
                )?;
                write_raw_to_dest(ip, dest_dir, &bytes)
            })();
            results.push((ip.clone(), result));
        }
    }

    results
}
