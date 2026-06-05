use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::models::{BuildConfig, BuildMode, BuildTarget, FontReplacement, StagingArea, UEVersion};
use crate::locres::write_locres;

pub struct ModBuilder {
    pub staging: StagingArea,
    pub config: BuildConfig,
    pub source_paks: Vec<PathBuf>,
    pub aes_key: Option<String>,
}

// 工作目錄 RAII 守衛：build 流程中途失敗時（`?` 或 panic）仍會清理 TEMP 目錄
struct WorkDirGuard(PathBuf);

impl Drop for WorkDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl ModBuilder {
    fn prepare_work_dir(&self) -> Result<WorkDirGuard> {
        let work_dir = std::env::temp_dir()
            .join(format!("ue_mod_{}", self.config.mod_name));
        let _ = fs::remove_dir_all(&work_dir);
        fs::create_dir_all(&work_dir)?;
        Ok(WorkDirGuard(work_dir))
    }

    pub fn build(&self, log: &mut Vec<String>) -> Result<PathBuf> {
        let work_dir_guard = self.prepare_work_dir()?;
        let work_dir = work_dir_guard.0.as_path();

        // 因 BuildTarget 現在是 Copy，不需要 .clone()
        let target = self.config.target;

        if matches!(target, BuildTarget::All | BuildTarget::LocresOnly) {
            if !self.staging.locres_edits.is_empty() {
                log.push(format!("處理 {} 個 locres 檔案...", self.staging.locres_edits.len()));
                self.apply_locres_edits(work_dir, log)?;
            }
        }

        if matches!(target, BuildTarget::All | BuildTarget::FontsOnly) {
            if !self.staging.font_replacements.is_empty() {
                log.push(format!("處理 {} 個字體替換...", self.staging.font_replacements.len()));
                self.apply_font_replacements(work_dir, log)?;
            }
        }

        let output_path = self.pack(work_dir, log)?;
        // 成功路徑：work_dir_guard 在離開 scope 時自動清除

        log.push(format!("完成: {}", output_path.display()));
        Ok(output_path)
    }

    fn apply_locres_edits(&self, work_dir: &Path, log: &mut Vec<String>) -> Result<()> {
        // 由於 mod .pak 同一個內部路徑只能有一份檔案，多個 (source_pak, path)
        // 共享相同 internal_path 時最終只會留下最後一筆。先做去重檢查並警示。
        let mut seen: std::collections::HashMap<&str, &Path> = std::collections::HashMap::new();

        for ((source_pak, pak_path), entries) in &self.staging.locres_edits {
            let modified_count = entries.iter().filter(|e| e.is_modified()).count();
            if modified_count == 0 {
                continue;
            }

            if let Some(prev) = seen.insert(pak_path.as_str(), source_pak.as_path()) {
                log.push(format!(
                    "  ⚠ 多個來源使用同一內部路徑 {}（{} 與 {}）→ 後處理的會覆蓋前者，建議分成多個 mod。",
                    pak_path,
                    prev.file_name().unwrap_or_default().to_string_lossy(),
                    source_pak.file_name().unwrap_or_default().to_string_lossy(),
                ));
            }

            let extracted_path =
                work_dir.join(pak_path.replace('/', std::path::MAIN_SEPARATOR_STR));

            // 優先從紀錄的來源 pak 抽；抽不到（pak 已搬走 / 名稱變了）才退回搜整堆 source_paks。
            let aes = self.aes_key.as_deref();
            let used_pak: PathBuf = if crate::locres::extract_locres_to_file(
                source_pak, pak_path, &extracted_path, aes,
            ).is_ok() {
                source_pak.clone()
            } else if let Some(fallback) = self.extract_first_match(pak_path, &extracted_path) {
                log.push(format!(
                    "  ⚠ 紀錄來源 {} 抽取失敗，退回從 {} 抽取。",
                    source_pak.file_name().unwrap_or_default().to_string_lossy(),
                    fallback.file_name().unwrap_or_default().to_string_lossy(),
                ));
                fallback.to_path_buf()
            } else {
                anyhow::bail!("找不到包含 {} 的 pak（嘗試了紀錄來源與全部 source_paks）", pak_path);
            };

            log.push(format!(
                "  處理: {} (來源: {})",
                pak_path,
                used_pak.file_name().unwrap_or_default().to_string_lossy()
            ));

            write_locres(entries, &extracted_path, &extracted_path)?;
            log.push(format!("  寫入 {} 處修改", modified_count));
        }
        Ok(())
    }

    fn apply_font_replacements(&self, work_dir: &Path, log: &mut Vec<String>) -> Result<()> {
        // 與 locres 同樣：mod pak 內部路徑唯一，多個來源映射同一路徑時警示。
        let mut seen: std::collections::HashMap<&str, &Path> = std::collections::HashMap::new();

        for FontReplacement { source_pak, pak_path, replacement } in &self.staging.font_replacements {
            if let Some(prev) = seen.insert(pak_path.as_str(), source_pak.as_path()) {
                log.push(format!(
                    "  ⚠ 多個字體來源使用同一內部路徑 {}（{} 與 {}）→ 後者會覆蓋前者。",
                    pak_path,
                    prev.file_name().unwrap_or_default().to_string_lossy(),
                    source_pak.file_name().unwrap_or_default().to_string_lossy(),
                ));
            }

            let dest = work_dir.join(pak_path.replace('/', std::path::MAIN_SEPARATOR_STR));
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(replacement, &dest)?;
            log.push(format!(
                "  字體替換: {} <- {} (源自 {})",
                pak_path,
                replacement.display(),
                source_pak.file_name().unwrap_or_default().to_string_lossy(),
            ));
        }
        Ok(())
    }

    /// 依序嘗試每個來源 PAK，回傳第一個成功提取到 `internal_path` 的 PAK。
    /// pakchunk0 優先（通常為主資源 PAK），其餘按 source_paks 順序。
    fn extract_first_match(&self, internal_path: &str, output: &Path) -> Option<&Path> {
        let chunk0_idx = self.source_paks.iter().position(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_ascii_lowercase()
                .starts_with("pakchunk0")
        });

        let aes = self.aes_key.as_deref();
        let try_extract = |p: &Path| -> bool {
            crate::locres::extract_locres_to_file(p, internal_path, output, aes).is_ok()
        };

        if let Some(i) = chunk0_idx {
            let p = self.source_paks[i].as_path();
            if try_extract(p) {
                return Some(p);
            }
        }
        for (i, candidate) in self.source_paks.iter().enumerate() {
            if Some(i) == chunk0_idx {
                continue;
            }
            let p = candidate.as_path();
            if try_extract(p) {
                return Some(p);
            }
        }
        None
    }

    fn pack(&self, work_dir: &Path, log: &mut Vec<String>) -> Result<PathBuf> {
        fs::create_dir_all(&self.config.output_dir)?;

        let suffix = match self.config.target {
            BuildTarget::LocresOnly => "_Locres",
            BuildTarget::FontsOnly  => "_Fonts",
            BuildTarget::All        => "",
        };

        let output_pak = self
            .config
            .output_dir
            .join(format!("{}{}_P.pak", self.config.mod_name, suffix));

        let repak_ver_str = self.config.version.repak_version();
        let repak_ver = match repak_ver_str {
            "V2"  => repak::Version::V2,
            "V3"  => repak::Version::V3,
            "V4"  => repak::Version::V4,
            "V5"  => repak::Version::V5,
            "V7"  => repak::Version::V7,
            "V8A" => repak::Version::V8A,
            "V8B" => repak::Version::V8B,
            "V9"  => repak::Version::V9,
            _     => repak::Version::V11,
        };

        // UE4.27 以上支援 Oodle
        let (compression_str, repak_compression) = match self.config.version.repak_compression() {
            "Oodle" => ("Oodle", repak::Compression::Oodle),
            _       => ("Zlib",  repak::Compression::Zlib),
        };

        log.push(format!(
            "打包 PAK: {} (版本: {}, 壓縮: {})",
            output_pak.display(),
            repak_ver_str,
            compression_str
        ));

        {
            let out_file = fs::File::create(&output_pak).context("無法建立輸出的 PAK 檔案")?;
            let mut out_writer = std::io::BufWriter::new(out_file);

            let builder = repak::PakBuilder::new().compression(vec![repak_compression]);
            let mut writer =
                builder.writer(&mut out_writer, repak_ver, "../../../".to_string(), None);

            for entry in walkdir::WalkDir::new(work_dir) {
                let entry = entry?;
                if entry.file_type().is_file() {
                    let path = entry.path();
                    let rel_path = path
                        .strip_prefix(work_dir)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");
                    let data = fs::read(path)?;
                    writer
                        .write_file(&rel_path, true, &data)
                        .context("寫入 pak 檔案失敗")?;
                }
            }
            writer.write_index().context("寫入 pak 索引失敗")?;
        }

        if self.config.mode == BuildMode::IoStore {
            let output_utoc = output_pak.with_extension("utoc");
            log.push("產生偽裝 IoStore 檔頭 (.utoc / .ucas)".to_string());

            let engine_version = match self.config.version {
                UEVersion::UE4_25 => retoc::version::EngineVersion::UE4_25,
                UEVersion::UE4_26 => retoc::version::EngineVersion::UE4_26,
                UEVersion::UE4_27 => retoc::version::EngineVersion::UE4_27,
                UEVersion::UE5_0  => retoc::version::EngineVersion::UE5_0,
                UEVersion::UE5_1  => retoc::version::EngineVersion::UE5_1,
                UEVersion::UE5_2  => retoc::version::EngineVersion::UE5_2,
                UEVersion::UE5_3  => retoc::version::EngineVersion::UE5_3,
                UEVersion::UE5_4  => retoc::version::EngineVersion::UE5_4,
                UEVersion::UE5_5  => retoc::version::EngineVersion::UE5_5,
                UEVersion::UE5_6  => retoc::version::EngineVersion::UE5_6,
                UEVersion::UE5_7  => retoc::version::EngineVersion::UE5_7,

                // For Future (5.8+) 自動降級使用最高支援的 5.7 IoStore (Retco)
                v if v >= UEVersion::UE5_8 => {
                    log.push("提示：UE 5.8+ 目前使用 5.7 的 IoStore 結構進行打包".to_string());
                    retoc::version::EngineVersion::UE5_7
                },
                _ => retoc::version::EngineVersion::UE5_0,
            };

            let mount_point = retoc::UEPath::new("../../../");
            let io_writer = retoc::iostore_writer::IoStoreWriter::new(
                &output_utoc,
                engine_version.toc_version(),
                Some(engine_version.container_header_version()),
                mount_point.into(),
            )
            .context("無法建立 IoStoreWriter")?;

            io_writer.finalize().context("無法 Finalize IoStoreWriter")?;
        }

        Ok(output_pak)
    }
}