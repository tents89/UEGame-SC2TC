// 外部資源打包：使用者直接選一個資料夾，把裡面整包打成 mod。
//
// 兩種模式：
//   * BuildMode::Pak     → 用 repak 把整個資料夾打成 .pak（與舊版 UE 相容）。
//   * BuildMode::IoStore → 參考 retoc 的 `to-zen`：把 `.uasset` / `.umap`
//     （需有配對的 `.uexp`）轉成 zen 格式寫進 IoStoreWriter；其餘檔案
//     （含沒有 .uexp 的 .uasset、設定、字體、ini 等）改用 repak 落地到 .pak。
//
// 路徑映射：使用者選擇的「來源資料夾」就是 pak 內部路徑根。例如資料夾裡
// 的 `MyGame/Content/Foo.uasset` 會以 `MyGame/Content/Foo.uasset` 寫入容器。

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::models::{BuildMode, UEVersion};

#[derive(Clone, Debug)]
pub struct ExternalPackConfig {
    pub source_dir: PathBuf,
    pub output_dir: PathBuf,
    pub mod_name: String,
    pub mode: BuildMode,
    pub version: UEVersion,
}

pub struct ExternalPackOutput {
    pub pak_path: PathBuf,
    pub utoc_path: Option<PathBuf>,
    pub ucas_path: Option<PathBuf>,
}

pub fn build_external(config: &ExternalPackConfig, log: &mut Vec<String>) -> Result<ExternalPackOutput> {
    if !config.source_dir.is_dir() {
        anyhow::bail!("來源目錄不存在或不是資料夾: {}", config.source_dir.display());
    }
    fs::create_dir_all(&config.output_dir)
        .with_context(|| format!("無法建立輸出目錄: {}", config.output_dir.display()))?;

    let suffix = "_External";
    let output_pak = config
        .output_dir
        .join(format!("{}{}_P.pak", config.mod_name, suffix));

    match config.mode {
        BuildMode::Pak => {
            pack_all_to_pak(&config.source_dir, &output_pak, config.version, log)?;
            log.push(format!("PAK 模式完成: {}", output_pak.display()));
            Ok(ExternalPackOutput { pak_path: output_pak, utoc_path: None, ucas_path: None })
        }
        BuildMode::IoStore => {
            let utoc_path = output_pak.with_extension("utoc");
            let ucas_path = output_pak.with_extension("ucas");
            pack_iostore_with_pak_fallback(&config.source_dir, &output_pak, &utoc_path, config.version, log)?;
            log.push(format!("IoStore 模式完成: {} (+ .utoc / .ucas)", output_pak.display()));
            Ok(ExternalPackOutput {
                pak_path: output_pak,
                utoc_path: Some(utoc_path),
                ucas_path: Some(ucas_path),
            })
        }
    }
}

// ── 收集相對路徑 ─────────────────────────────────────────────────────────

fn collect_files(root: &Path) -> Result<Vec<(PathBuf, String)>> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(root) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let abs = entry.path().to_path_buf();
        let rel = abs.strip_prefix(root)?;
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        out.push((abs, rel_str));
    }
    Ok(out)
}

fn repak_version(v: UEVersion) -> repak::Version {
    match v.repak_version() {
        "V2"  => repak::Version::V2,
        "V3"  => repak::Version::V3,
        "V4"  => repak::Version::V4,
        "V5"  => repak::Version::V5,
        "V7"  => repak::Version::V7,
        "V8A" => repak::Version::V8A,
        "V8B" => repak::Version::V8B,
        "V9"  => repak::Version::V9,
        _     => repak::Version::V11,
    }
}

fn repak_compression(v: UEVersion) -> repak::Compression {
    match v.repak_compression() {
        "Oodle" => repak::Compression::Oodle,
        _       => repak::Compression::Zlib,
    }
}

// ── PAK 模式 ────────────────────────────────────────────────────────────

fn pack_all_to_pak(
    source_dir: &Path,
    output_pak: &Path,
    version: UEVersion,
    log: &mut Vec<String>,
) -> Result<()> {
    let files = collect_files(source_dir)?;
    log.push(format!("PAK 模式：將 {} 個檔案打包至 {}", files.len(), output_pak.display()));

    let out_file = fs::File::create(output_pak)
        .with_context(|| format!("無法建立輸出的 PAK: {}", output_pak.display()))?;
    let mut out_writer = std::io::BufWriter::new(out_file);

    let builder = repak::PakBuilder::new().compression(vec![repak_compression(version)]);
    let mut writer = builder.writer(
        &mut out_writer,
        repak_version(version),
        "../../../".to_string(),
        None,
    );

    for (abs, rel) in files {
        let data = fs::read(&abs).with_context(|| format!("讀取失敗: {}", abs.display()))?;
        writer.write_file(&rel, true, &data)
            .with_context(|| format!("寫入 pak 失敗: {}", rel))?;
    }
    writer.write_index().context("寫入 pak 索引失敗")?;
    Ok(())
}

// ── IoStore 模式 ───────────────────────────────────────────────────────

const ASSET_SIBLING_EXTS: &[&str] = &["uexp", "ubulk", "uptnl"];

fn read_opt(p: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(p) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn engine_version_for(version: UEVersion) -> retoc::version::EngineVersion {
    use retoc::version::EngineVersion as E;
    match version {
        UEVersion::UE4_25 => E::UE4_25,
        UEVersion::UE4_26 => E::UE4_26,
        UEVersion::UE4_27 => E::UE4_27,
        UEVersion::UE5_0  => E::UE5_0,
        UEVersion::UE5_1  => E::UE5_1,
        UEVersion::UE5_2  => E::UE5_2,
        UEVersion::UE5_3  => E::UE5_3,
        UEVersion::UE5_4  => E::UE5_4,
        UEVersion::UE5_5  => E::UE5_5,
        UEVersion::UE5_6  => E::UE5_6,
        UEVersion::UE5_7  => E::UE5_7,
        v if v >= UEVersion::UE5_8 => E::UE5_7,
        _ => E::UE5_0,
    }
}

fn pack_iostore_with_pak_fallback(
    source_dir: &Path,
    output_pak: &Path,
    output_utoc: &Path,
    version: UEVersion,
    log: &mut Vec<String>,
) -> Result<()> {
    let files = collect_files(source_dir)?;

    // 對應 retoc to-zen 的篩選：把有配對 .uexp 的 .uasset / .umap 轉成 zen。
    let rel_set: HashSet<String> = files.iter().map(|(_, r)| r.clone()).collect();
    let mut zen_assets: Vec<(PathBuf, String)> = Vec::new(); // 給 IoStore 的 .uasset / .umap
    let mut consumed: HashSet<String> = HashSet::new();      // 已被 zen 流程消耗的相對路徑

    for (abs, rel) in &files {
        let lower = rel.to_ascii_lowercase();
        let is_asset = lower.ends_with(".uasset") || lower.ends_with(".umap");
        if !is_asset {
            continue;
        }
        let stem_rel = rel.rsplit_once('.').map(|x| x.0).unwrap_or(rel.as_str());
        let uexp_rel = format!("{}.uexp", stem_rel);
        if rel_set.contains(&uexp_rel) {
            zen_assets.push((abs.clone(), rel.clone()));
            consumed.insert(rel.clone());
            consumed.insert(uexp_rel);
            for ext in ASSET_SIBLING_EXTS.iter().chain(std::iter::once(&"m.ubulk")) {
                let sib = format!("{}.{}", stem_rel, ext);
                if rel_set.contains(&sib) {
                    consumed.insert(sib);
                }
            }
        }
    }

    log.push(format!(
        "IoStore 模式：{} 個資產 → zen 容器，其餘 {} 個檔案 → .pak",
        zen_assets.len(),
        files.len() - consumed.len() - zen_assets.iter().filter(|(_, r)| consumed.contains(r)).count(),
    ));

    // ── 1. 寫 IoStore（.utoc + .ucas）────────────────────────────────────
    write_iostore(source_dir, output_utoc, &zen_assets, version, log)?;

    // ── 2. 寫 PAK（被排除的檔案）──────────────────────────────────────────
    let pak_files: Vec<(PathBuf, String)> = files
        .into_iter()
        .filter(|(_, rel)| !consumed.contains(rel))
        .collect();

    let out_file = fs::File::create(output_pak)
        .with_context(|| format!("無法建立輸出的 PAK: {}", output_pak.display()))?;
    let mut out_writer = std::io::BufWriter::new(out_file);

    let builder = repak::PakBuilder::new().compression(vec![repak_compression(version)]);
    let mut writer = builder.writer(
        &mut out_writer,
        repak_version(version),
        "../../../".to_string(),
        None,
    );

    if pak_files.is_empty() {
        // 空 .pak 仍需建立，否則遊戲不會載入 .utoc / .ucas（與 retoc to-zen 行為一致）。
        log.push("PAK：無附加檔案，輸出空 .pak 以滿足 IoStore 容器載入要求".to_string());
    } else {
        for (abs, rel) in pak_files {
            let data = fs::read(&abs).with_context(|| format!("讀取失敗: {}", abs.display()))?;
            writer.write_file(&rel, true, &data)
                .with_context(|| format!("寫入 pak 失敗: {}", rel))?;
        }
    }
    writer.write_index().context("寫入 pak 索引失敗")?;

    Ok(())
}

fn write_iostore(
    source_dir: &Path,
    output_utoc: &Path,
    zen_assets: &[(PathBuf, String)],
    version: UEVersion,
    log: &mut Vec<String>,
) -> Result<()> {
    use retoc::iostore_writer::IoStoreWriter;
    use retoc::legacy_asset::FSerializedAssetBundle;
    use retoc::logging::Log;
    use retoc::zen_asset_conversion;
    use retoc::UEPath;

    let engine_version = engine_version_for(version);
    let mount_point = UEPath::new("../../../");

    let mut writer = IoStoreWriter::new(
        output_utoc,
        engine_version.toc_version(),
        Some(engine_version.container_header_version()),
        mount_point.into(),
    )
    .context("無法建立 IoStoreWriter")?;

    if zen_assets.is_empty() {
        // 沒資產也要 finalize 出空容器，遊戲才會載入。
        writer.finalize().context("無法 Finalize IoStoreWriter")?;
        return Ok(());
    }

    // 若來源目錄帶有 scriptobjects.bin，按 retoc to-zen 行為當作 ScriptObjects 來源。
    let script_objects_path = source_dir.join("scriptobjects.bin");
    let script_objects = if script_objects_path.exists() {
        let buf = fs::read(&script_objects_path)?;
        let mut cur = std::io::Cursor::new(buf);
        let so = retoc::script_objects::ZenScriptObjects::deserialize_new(&mut cur)
            .context("scriptobjects.bin 解析失敗")?;
        Some(std::sync::Arc::new(so))
    } else {
        None
    };

    let log_backend = Log::no_log();
    let container_header_version = writer.container_header_version();
    let empty_shader_map: std::collections::HashMap<String, Vec<retoc::FSHAHash>> =
        std::collections::HashMap::new();

    // 不做跨 package external arc fixup（allow_fixup=false）。
    // UE5.0+ 因為 container header version > Initial 本就不需要，UE4 則可能在
    // 多檔案交叉引用時有 import 解析誤差；單一資產或自包含 mod 沒問題。
    let allow_fixup = false;

    for (abs, rel) in zen_assets {
        let asset = fs::read(abs).with_context(|| format!("讀取失敗: {}", abs.display()))?;
        let stem = rel.rsplit_once('.').map(|x| x.0).unwrap_or(rel.as_str());
        let uexp = fs::read(source_dir.join(format!("{}.uexp", stem)))
            .with_context(|| format!("讀取 .uexp 失敗: {}.uexp", stem))?;
        let ubulk = read_opt(&source_dir.join(format!("{}.ubulk", stem)))?;
        let uptnl = read_opt(&source_dir.join(format!("{}.uptnl", stem)))?;
        let m_ubulk = read_opt(&source_dir.join(format!("{}.m.ubulk", stem)))?;

        let bundle = FSerializedAssetBundle {
            asset_file_buffer: asset,
            exports_file_buffer: uexp,
            bulk_data_buffer: ubulk,
            optional_bulk_data_buffer: uptnl,
            memory_mapped_bulk_data_buffer: m_ubulk,
        };

        let path_in_container_str = format!("../../../{}", rel);
        let path_in_container = UEPath::new(&path_in_container_str);

        match zen_asset_conversion::build_zen_asset(
            bundle,
            &empty_shader_map,
            path_in_container,
            Some(engine_version.package_file_version()),
            container_header_version,
            allow_fixup,
            script_objects.clone(),
            None,
            &log_backend,
        ) {
            Ok(mut converted) => {
                converted.write(&mut writer)
                    .with_context(|| format!("寫入 zen 容器失敗: {}", rel))?;
            }
            Err(e) => {
                log.push(format!("資產轉換失敗，已跳過（會留在 .pak 端的 fallback）: {} ({:#})", rel, e));
            }
        }
    }

    writer.finalize().context("無法 Finalize IoStoreWriter")?;
    Ok(())
}
