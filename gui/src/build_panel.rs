use std::path::PathBuf;
use egui::*;
use ue_mod_core::*;
use ue_mod_core::staging::ModBuilder;

#[derive(Default)]
pub struct BuildPanelState {
    pub is_building: bool,
    pub build_log: Vec<String>,
    pub last_output: Option<PathBuf>,
    pub game_name: String,
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
    ui.separator();

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

    let can_build = !staging.is_empty() && output_dir.is_some() && !mod_name.is_empty();

    let actual_version = if *manual_override { *selected_version } else { *detected_version };
    let actual_mode = if *manual_override {
        match actual_version {
            v if v >= UEVersion::UE5_0 => BuildMode::IoStore,
            _ => BuildMode::Pak,
        }
    } else {
        detected_mode.clone()
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
                mode: actual_mode.clone(),
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

        let mut mods_dir = PathBuf::from("未知的 ~mods 目錄");
        if let Some(first_pak) = source_paks.first() {
            if let Some(paks_dir) = first_pak.parent() {
                mods_dir = paks_dir.join("~mods");
            }
        }

        ui.label("請在遊戲的 Paks 目錄下確認是否存在 `~mods` 資料夾 (如果沒有請手動建立)，並將生成的檔案放入：");
        ui.horizontal(|ui| {
            ui.label(RichText::new(mods_dir.display().to_string()).strong().color(Color32::from_rgb(200, 220, 255)));
            if ui.button("複製路徑").clicked() {
                ui.output_mut(|o| o.copied_text = mods_dir.display().to_string());
            }
        });

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