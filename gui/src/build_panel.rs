use std::path::PathBuf;
use egui::*;
use ue_mod_core::*;
use ue_mod_core::staging::ModBuilder;
use ue_mod_core::external_pack::{ExternalPackConfig, build_external};

#[derive(PartialEq, Clone, Copy)]
pub enum BuildSubTab {
    Game,
    External,
}

impl Default for BuildSubTab {
    fn default() -> Self { BuildSubTab::Game }
}

pub struct BuildPanelState {
    pub is_building: bool,
    pub last_output: Option<PathBuf>,
    pub game_name: String,

    // ── 子分頁 ────────────────────────────────────────────────────────────
    pub sub_tab: BuildSubTab,

    // ── 外部資源打包 ──────────────────────────────────────────────────────
    pub ext_source_dir: Option<PathBuf>,
    pub ext_output_dir: Option<PathBuf>,
    pub ext_mod_name: String,
    pub ext_version: UEVersion,
    pub ext_mode: BuildMode,
    pub ext_last_output: Option<PathBuf>,
    pub ext_last_utoc: Option<PathBuf>,
    pub ext_last_ucas: Option<PathBuf>,
}

impl Default for BuildPanelState {
    fn default() -> Self {
        Self {
            is_building: false,
            last_output: None,
            game_name: String::new(),
            sub_tab: BuildSubTab::default(),
            ext_source_dir: None,
            ext_output_dir: None,
            ext_mod_name: "ExternalMod".to_string(),
            ext_version: UEVersion::UE5_3,
            ext_mode: BuildMode::IoStore,
            ext_last_output: None,
            ext_last_utoc: None,
            ext_last_ucas: None,
        }
    }
}

pub fn show_build_panel(
    ui: &mut Ui,
    state: &mut BuildPanelState,
    staging: &mut StagingArea,
    mod_name: &mut String,
    output_dir: &mut Option<PathBuf>,
    detected_mode: &mut BuildMode,
    detected_version: &mut UEVersion,
    manual_override: &mut bool,
    selected_version: &mut UEVersion,
    source_paks: &[PathBuf],
    app_log: &mut Vec<LogEntry>,
    aes_key: &str,
) {
    ui.heading("建構 Mod");

    ui.horizontal(|ui| {
        ui.selectable_value(&mut state.sub_tab, BuildSubTab::Game, "遊戲內資源（在地化 / 字體）");
        ui.selectable_value(&mut state.sub_tab, BuildSubTab::External, "外部資源打包");
    });
    ui.separator();

    match state.sub_tab {
        BuildSubTab::Game => show_game_build(
            ui, state, staging, mod_name, output_dir,
            detected_mode, detected_version, manual_override, selected_version,
            source_paks, app_log, aes_key,
        ),
        BuildSubTab::External => show_external_build(ui, state, app_log),
    }
}

fn show_game_build(
    ui: &mut Ui,
    state: &mut BuildPanelState,
    staging: &mut StagingArea,
    mod_name: &mut String,
    output_dir: &mut Option<PathBuf>,
    detected_mode: &mut BuildMode,
    detected_version: &mut UEVersion,
    manual_override: &mut bool,
    selected_version: &mut UEVersion,
    source_paks: &[PathBuf],
    app_log: &mut Vec<LogEntry>,
    aes_key: &str,
) {
    ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
    Grid::new("build_config").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        ui.label("Mod 名稱:");
        ui.text_edit_singleline(mod_name);
        ui.end_row();

        ui.label("遊戲名稱:");
        ui.label(RichText::new(if state.game_name.is_empty() { "未偵測到" } else { &state.game_name }).strong());
        ui.end_row();

        ui.label("輸出目錄:");
        ui.horizontal(|ui| {
            let label = output_dir.as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or("請先選擇輸出目錄".to_string());
                
            if ui.add(Button::new(RichText::new("瀏覽...").strong().color(Color32::BLACK))
                    .fill(Color32::from_rgb(100, 200, 255))
                    .min_size(vec2(80.0, 24.0))).clicked() {
                *output_dir = rfd::FileDialog::new().pick_folder();
            }
            ui.label(label);
        });
        ui.end_row();

        ui.label("引擎版本:");
        ui.horizontal(|ui| {
            let mode_label = match detected_mode {
                BuildMode::IoStore => "IoStore (UE5)",
                BuildMode::Pak => "PAK (UE4)",
            };
            ui.colored_label(Color32::from_rgb(150, 200, 255), format!("自動偵測: {} ({})", detected_version.as_str(), mode_label));
            ui.checkbox(manual_override, "手動覆蓋");
        });
        ui.end_row();

        if *manual_override {
            ui.label("手動版本:");
            ComboBox::from_id_source("version_select")
                .selected_text(selected_version.to_string())
                .show_ui(ui, |ui| {
                    for v in UEVersion::all() {
                        ui.selectable_value(selected_version, *v, v.to_string());
                    }
                });
            ui.end_row();
        }
    });

    ui.separator();
    ui.heading("待建構區摘要");

    if staging.is_empty() {
        ui.colored_label(Color32::GRAY, "沒有任何修改。請先在檔案頁面中進行在地化編輯或字體替換。");
    } else {
        let total = staging.total_changes();
        ui.colored_label(Color32::from_rgb(100, 220, 100), format!("{} 個修改待建構", total));

        if !staging.locres_edits.is_empty() {
            ui.label(format!("  在地化修改: {} 個檔案", staging.locres_edits.len()));
        }
        if !staging.font_replacements.is_empty() {
            ui.label(format!("  字體替換: {} 個", staging.font_replacements.len()));
        }
    }

    ui.separator();

    // 跨平台檔名/目錄名禁用字元（Windows 最嚴格，Linux/macOS 雖較寬鬆，但統一禁用更安全）
    const FORBIDDEN_MOD_NAME_CHARS: &[char] = &['/', '\\', '<', '>', ':', '"', '|', '?', '*'];
    let mod_name_invalid_char =
        mod_name.chars().find(|c| FORBIDDEN_MOD_NAME_CHARS.contains(c));

    if let Some(c) = mod_name_invalid_char {
        ui.colored_label(
            Color32::from_rgb(255, 80, 80),
            format!("Mod 名稱不可含 / \\ < > : \" | ? * 等字元（目前含：'{}'）", c),
        );
    }

    let can_build = !staging.is_empty()
        && output_dir.is_some()
        && !mod_name.is_empty()
        && mod_name_invalid_char.is_none();

    let actual_version = if *manual_override { *selected_version } else { *detected_version };
    let actual_mode = if *manual_override {
        match actual_version {
            v if v >= UEVersion::UE5_0 => BuildMode::IoStore,
            _ => BuildMode::Pak,
        }
    } else {
        *detected_mode
    };

    ui.horizontal(|ui| {
        let mut trigger_build = None;

        if !staging.locres_edits.is_empty() {
            if ui.add_enabled(can_build && !state.is_building, Button::new("建立 文本 Mod")).clicked() {
                trigger_build = Some(BuildTarget::LocresOnly);
            }
        }
        
        if !staging.font_replacements.is_empty() {
            if ui.add_enabled(can_build && !state.is_building, Button::new("建立 字體 Mod")).clicked() {
                trigger_build = Some(BuildTarget::FontsOnly);
            }
        }

        if !staging.locres_edits.is_empty() && !staging.font_replacements.is_empty() {
            if ui.add_enabled(can_build && !state.is_building, Button::new("建立 完整 Mod").fill(Color32::from_rgb(50, 120, 50))).clicked() {
                trigger_build = Some(BuildTarget::All);
            }
        }

        if let Some(target) = trigger_build {
            let config = BuildConfig {
                mode: actual_mode,
                version: actual_version,
                output_dir: output_dir.clone().unwrap(),
                mod_name: mod_name.clone(),
                target,
            };

            let mut build_log = vec![];
            let builder = ModBuilder {
                staging: staging.clone(),
                config,
                source_paks: source_paks.to_vec(),
                aes_key: if aes_key.is_empty() { None } else { Some(aes_key.to_string()) },
            };

            match builder.build(&mut build_log) {
                Ok(output) => {
                    state.last_output = Some(output.clone());
                    app_log.push(LogEntry {
                        level: LogLevel::Success,
                        message: format!("建構成功: {}", output.display()),
                        timestamp: std::time::SystemTime::now(),
                    });
                }
                Err(e) => {
                    app_log.push(LogEntry {
                        level: LogLevel::Error,
                        message: format!("建構失敗: {}", e),
                        timestamp: std::time::SystemTime::now(),
                    });
                }
            }

            for line in build_log {
                app_log.push(LogEntry { level: LogLevel::Info, message: line, timestamp: std::time::SystemTime::now() });
            }
        }
    });

    if let Some(output) = &state.last_output {
        ui.separator();
        ui.heading(RichText::new("建構成功！ 安裝指南").color(Color32::from_rgb(100, 255, 100)));
        ui.add_space(4.0);

        let paks_dir = source_paks.first().and_then(|p| p.parent());

        ui.label("請打開遊戲的 Paks 目錄，若不存在 `~mods` 子資料夾請手動建立，並將生成的檔案放入其中：");
        ui.horizontal(|ui| {
            match paks_dir {
                Some(dir) => {
                    ui.label(
                        RichText::new(dir.display().to_string())
                            .strong()
                            .color(Color32::from_rgb(200, 220, 255)),
                    );
                    if ui.button("複製 Paks 路徑").clicked() {
                        ui.output_mut(|o| o.copied_text = dir.display().to_string());
                    }
                }
                None => {
                    ui.colored_label(Color32::GRAY, "(尚未掃描來源 PAK，無法顯示路徑)");
                }
            }
        });
        ui.label(
            RichText::new("提示：`~mods` 是 Unreal Engine 約定的 mod 載入子目錄，名稱不可修改。")
                .small()
                .color(Color32::GRAY),
        );

        ui.add_space(4.0);
        ui.label("需要放入的檔案包含：");
        ui.label(format!(" - {}", output.file_name().unwrap_or_default().to_string_lossy()));

        if actual_mode == BuildMode::IoStore {
            let utoc = output.with_extension("utoc");
            let ucas = output.with_extension("ucas");
            ui.label(format!(" - {}", utoc.file_name().unwrap_or_default().to_string_lossy()));
            ui.label(format!(" - {}", ucas.file_name().unwrap_or_default().to_string_lossy()));
            ui.add_space(4.0);
            ui.label(RichText::new("注意：IoStore 模式必須同時放入 .pak + .utoc + .ucas 這三個檔案才會生效！").color(Color32::from_rgb(255, 200, 50)));
        }
    }

    if !app_log.is_empty() {
        ui.separator();
        ui.label("建構日誌:");
        ScrollArea::vertical().max_height(200.0).stick_to_bottom(true).show(ui, |ui| {
            for entry in app_log.iter().rev().take(50) {
                let color = match entry.level {
                    LogLevel::Success => Color32::from_rgb(100, 220, 100),
                    LogLevel::Error => Color32::from_rgb(255, 80, 80),
                    LogLevel::Warning => Color32::from_rgb(255, 200, 50),
                    LogLevel::Info => Color32::from_gray(180),
                };
                ui.colored_label(color, &entry.message);
            }
        });
    }
    }); // end ScrollArea::both()
}

// ── 外部資源打包 ────────────────────────────────────────────────────────────
fn show_external_build(
    ui: &mut Ui,
    state: &mut BuildPanelState,
    app_log: &mut Vec<LogEntry>,
) {
    ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
        ui.label(
            RichText::new(
                "把外部資料夾整包打成 mod。此分頁與遊戲資源無關，僅依賴使用者提供的檔案。"
            )
            .color(Color32::from_rgb(180, 200, 255)),
        );
        ui.add_space(4.0);

        Grid::new("ext_build_config").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label("Mod 名稱:");
            ui.text_edit_singleline(&mut state.ext_mod_name);
            ui.end_row();

            ui.label("來源資料夾:");
            ui.horizontal(|ui| {
                if ui.button("瀏覽...").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        state.ext_source_dir = Some(p);
                    }
                }
                let label = state.ext_source_dir.as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "(尚未選擇)".to_string());
                ui.label(label);
            });
            ui.end_row();

            ui.label("輸出目錄:");
            ui.horizontal(|ui| {
                if ui.button("瀏覽...").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        state.ext_output_dir = Some(p);
                    }
                }
                let label = state.ext_output_dir.as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "(尚未選擇)".to_string());
                ui.label(label);
            });
            ui.end_row();

            ui.label("引擎版本:");
            ComboBox::from_id_source("ext_version_select")
                .selected_text(state.ext_version.to_string())
                .show_ui(ui, |ui| {
                    for v in UEVersion::all() {
                        ui.selectable_value(&mut state.ext_version, *v, v.to_string());
                    }
                });
            ui.end_row();

            ui.label("打包模式:");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut state.ext_mode, BuildMode::IoStore, "IoStore (UE5)");
                ui.selectable_value(&mut state.ext_mode, BuildMode::Pak,     "PAK (UE4)");
            });
            ui.end_row();
        });

        ui.add_space(4.0);
        ui.colored_label(
            Color32::GRAY,
            "說明：IoStore 模式下，有配對 .uexp 的 .uasset / .umap 會以 retoc 轉成 zen 寫入 .utoc/.ucas；\
             其餘檔案會以 repak 寫入 .pak。PAK 模式則直接把整個資料夾打成 .pak。",
        );

        ui.add_space(4.0);

        // 跨平台檔名禁用字元
        const FORBIDDEN: &[char] = &['/', '\\', '<', '>', ':', '"', '|', '?', '*'];
        let name_invalid = state.ext_mod_name.chars().find(|c| FORBIDDEN.contains(c));
        if let Some(c) = name_invalid {
            ui.colored_label(
                Color32::from_rgb(255, 80, 80),
                format!("Mod 名稱不可含 / \\ < > : \" | ? * 等字元（目前含：'{}'）", c),
            );
        }

        let can_build = state.ext_source_dir.is_some()
            && state.ext_output_dir.is_some()
            && !state.ext_mod_name.is_empty()
            && name_invalid.is_none();

        ui.horizontal(|ui| {
            if ui.add_enabled(can_build, Button::new("打包外部資源").fill(Color32::from_rgb(50, 120, 50))).clicked() {
                let cfg = ExternalPackConfig {
                    source_dir: state.ext_source_dir.clone().unwrap(),
                    output_dir: state.ext_output_dir.clone().unwrap(),
                    mod_name:   state.ext_mod_name.clone(),
                    mode:       state.ext_mode,
                    version:    state.ext_version,
                };

                let mut build_log = vec![];
                match build_external(&cfg, &mut build_log) {
                    Ok(out) => {
                        state.ext_last_output = Some(out.pak_path.clone());
                        state.ext_last_utoc = out.utoc_path.clone();
                        state.ext_last_ucas = out.ucas_path.clone();
                        app_log.push(LogEntry {
                            level: LogLevel::Success,
                            message: format!("外部資源打包成功: {}", out.pak_path.display()),
                            timestamp: std::time::SystemTime::now(),
                        });
                    }
                    Err(e) => {
                        app_log.push(LogEntry {
                            level: LogLevel::Error,
                            message: format!("外部資源打包失敗: {:#}", e),
                            timestamp: std::time::SystemTime::now(),
                        });
                    }
                }
                for line in build_log {
                    app_log.push(LogEntry {
                        level: LogLevel::Info,
                        message: line,
                        timestamp: std::time::SystemTime::now(),
                    });
                }
            }
        });

        if let Some(pak) = &state.ext_last_output {
            ui.separator();
            ui.heading(RichText::new("輸出檔案").color(Color32::from_rgb(100, 255, 100)));
            ui.label(format!(" - {}", pak.display()));
            if let Some(utoc) = &state.ext_last_utoc {
                ui.label(format!(" - {}", utoc.display()));
            }
            if let Some(ucas) = &state.ext_last_ucas {
                ui.label(format!(" - {}", ucas.display()));
            }
            if state.ext_mode == BuildMode::IoStore {
                ui.add_space(4.0);
                ui.label(
                    RichText::new("注意：IoStore 模式必須同時放入 .pak + .utoc + .ucas 這三個檔案才會生效！")
                        .color(Color32::from_rgb(255, 200, 50)),
                );
            }
        }

        if !app_log.is_empty() {
            ui.separator();
            ui.label("建構日誌:");
            ScrollArea::vertical().max_height(200.0).stick_to_bottom(true).show(ui, |ui| {
                for entry in app_log.iter().rev().take(50) {
                    let color = match entry.level {
                        LogLevel::Success => Color32::from_rgb(100, 220, 100),
                        LogLevel::Error   => Color32::from_rgb(255, 80, 80),
                        LogLevel::Warning => Color32::from_rgb(255, 200, 50),
                        LogLevel::Info    => Color32::from_gray(180),
                    };
                    ui.colored_label(color, &entry.message);
                }
            });
        }
    });
}