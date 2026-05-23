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

impl ModBuilder {
    fn prepare_work_dir(&self) -> Result<PathBuf> {
        let work_dir = std::env::temp_dir()
            .join(format!("ue_mod_{}", self.config.mod_name));
        let _ = fs::remove_dir_all(&work_dir);
        fs::create_dir_all(&work_dir)?;
        Ok(work_dir)
    }

    pub fn build(&self, log: &mut Vec<String>) -> Result<PathBuf> {
        let work_dir = self.prepare_work_dir()?;

        // 因 BuildTarget 現在是 Copy，不需要 .clone()
        let target = self.config.target;

        if matches!(target, BuildTarget::All | BuildTarget::LocresOnly) {
            if !self.staging.locres_edits.is_empty() {
                log.push(format!("處理 {} 個 locres 檔案...", self.staging.locres_edits.len()));
                self.apply_locres_edits(&work_dir, log)?;
            }
        }

        if matches!(target, BuildTarget::All | BuildTarget::FontsOnly) {
            if !self.staging.font_replacements.is_empty() {
                log.push(format!("處理 {} 個字體替換...", self.staging.font_replacements.len()));
                self.apply_font_replacements(&work_dir, log)?;
            }
        }

        let output_path = self.pack(&work_dir, log)?;
        let _ = fs::remove_dir_all(&work_dir);

        log.push(format!("完成: {}", output_path.display()));
        Ok(output_path)
    }

    fn apply_locres_edits(&self, work_dir: &Path, log: &mut Vec<String>) -> Result<()> {
        for (pak_path, entries) in &self.staging.locres_edits {
            let modified_count = entries.iter().filter(|e| e.is_modified()).count();
            if modified_count == 0 {
                continue;
            }

            let source_pak = self
                .find_source_pak(pak_path)
                .with_context(|| format!("找不到包含 {} 的 pak", pak_path))?;

            let extracted_path =
                work_dir.join(pak_path.replace('/', std::path::MAIN_SEPARATOR_STR));

            crate::locres::extract_locres_to_file(
                source_pak,
                pak_path,
                &extracted_path,
                self.aes_key.as_deref(),
            )?;
            log.push(format!("  處理: {}", pak_path));

            write_locres(entries, &extracted_path, &extracted_path)?;
            log.push(format!("  寫入 {} 處修改", modified_count));
        }
        Ok(())
    }

    fn apply_font_replacements(&self, work_dir: &Path, log: &mut Vec<String>) -> Result<()> {
        for FontReplacement { pak_path, replacement } in &self.staging.font_replacements {
            let dest = work_dir.join(pak_path.replace('/', std::path::MAIN_SEPARATOR_STR));
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(replacement, &dest)?;
            log.push(format!("  字體替換: {} <- {}", pak_path, replacement.display()));
        }
        Ok(())
    }

    fn find_source_pak(&self, _internal_path: &str) -> Option<&Path> {
        self.source_paks
            .iter()
            .find(|p| {
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .starts_with("pakchunk0")
            })
            .or_else(|| self.source_paks.first())
            .map(PathBuf::as_path)
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
            "V6"  => repak::Version::V6,
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