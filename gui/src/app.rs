use std::collections::BTreeMap;
use std::path::PathBuf;

use egui::*;
use ue_mod_core::*;

use crate::tree_view::TreeViewState;
use crate::locres_editor::LocresEditorState;
use crate::build_panel::BuildPanelState;

#[derive(PartialEq, Clone, Copy)]
pub enum ActivePanel {
    Files,
    LocresEditor,
    Build,
    Log,
    About,
}

#[derive(PartialEq, Clone, Copy)]
pub enum AesTab {
    Internal,
    External,
}

pub struct App {
    pub game_dir: Option<PathBuf>,
    pub shipping_exe_path: Option<PathBuf>,
    pub aes_key: String,

    pub all_paks: Vec<PathBuf>,
    pub tree_root: Vec<TreeNode>,
    pub scan_status: String,
    pub is_scanning: bool,
    pub active_panel: ActivePanel,
    pub tree_view: TreeViewState,
    pub locres_editor: LocresEditorState,
    pub build_panel: BuildPanelState,
    pub staging: StagingArea,
    pub log: Vec<LogEntry>,
    pub detected_mode: BuildMode,
    pub detected_version: UEVersion,
    pub detect_evidence: String,
    pub mod_name: String,
    pub output_dir: Option<PathBuf>,
    pub manual_version_override: bool,
    pub selected_version: UEVersion,

    pub show_multi_exe_warning: bool,
    pub show_aes_dialog: bool,
    pub aes_tab: AesTab,

    // WindowsNoEditor PAK 偵測流程
    pub show_exe_picker: bool,
    pub exe_picker_candidates: Vec<PathBuf>,
    pub exe_picker_selected: Option<usize>,
    pub show_wne_version_dialog: bool,
    pub wne_paks_need_aes: bool,
    pub wne_manual_version: Option<UEVersion>,
    pub wne_aes_input: String,
}

impl Default for App {
    fn default() -> Self {
        Self {
            game_dir: None,
            shipping_exe_path: None,
            aes_key: String::new(),
            all_paks: vec![],
            tree_root: vec![],
            scan_status: "尚未掃描".to_string(),
            is_scanning: false,
            active_panel: ActivePanel::Files,
            tree_view: TreeViewState::default(),
            locres_editor: LocresEditorState::default(),
            build_panel: BuildPanelState::default(),
            staging: StagingArea::default(),
            log: vec![],
            detected_mode: BuildMode::Pak,
            detected_version: UEVersion::UE4_27,
            detect_evidence: String::new(),
            mod_name: "CHT".to_string(),
            output_dir: None,
            manual_version_override: false,
            selected_version: UEVersion::UE4_27,
            show_multi_exe_warning: false,
            show_aes_dialog: false,
            aes_tab: AesTab::Internal,
            show_exe_picker: false,
            exe_picker_candidates: vec![],
            exe_picker_selected: None,
            show_wne_version_dialog: false,
            wne_paks_need_aes: false,
            wne_manual_version: None,
            wne_aes_input: String::new(),
        }
    }
}

impl App {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self::default()
    }

    fn push_log(&mut self, level: LogLevel, msg: impl Into<String>) {
        self.log.push(LogEntry {
            level,
            message: msg.into(),
            timestamp: std::time::SystemTime::now(),
        });
    }

    fn do_scan(&mut self) {
        let game_dir = match self.game_dir.clone() {
            Some(d) => d,
            None => {
                self.scan_status = "請先選擇遊戲目錄".to_string();
                return;
            }
        };

        // 搜尋 Shipping EXE
        let mut shipping_exes = vec![];
        for entry in walkdir::WalkDir::new(&game_dir).max_depth(5).into_iter().flatten() {
            let p = entry.path();
            if !p.extension().is_some_and(|ext| ext == "exe") { continue; }
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
            if p.to_string_lossy().contains("Binaries") && name.contains("shipping") {
                shipping_exes.push(p.to_path_buf());
            }
        }

        if shipping_exes.len() > 1 {
            self.scan_status = "錯誤：偵測到多個 Shipping.exe".to_string();
            self.push_log(LogLevel::Error, "目錄包含多個 Shipping.exe，已取消掃描。");
            self.game_dir = None;
            self.show_multi_exe_warning = true;
            return;
        }

        self.shipping_exe_path = shipping_exes.into_iter().next();

        // ── WindowsNoEditor PAK 流程：無 Shipping.exe 但有 WNE pak ──
        if self.shipping_exe_path.is_none() {
            let wne_paks = find_wne_paks(&game_dir);
            if !wne_paks.is_empty() {
                let all_exes = find_all_exes(&game_dir);
                if all_exes.is_empty() {
                    // 完全沒有 EXE → 直接跳版本選擇對話框
                    self.prepare_wne_version_dialog(&wne_paks);
                } else {
                    // 有 EXE → 先讓使用者挑選
                    self.exe_picker_candidates = all_exes;
                    self.exe_picker_selected = None;
                    self.show_exe_picker = true;
                    self.scan_status = "找到 WindowsNoEditor PAK，請選擇執行檔".to_string();
                }
                return;
            }
        }

        self.push_log(LogLevel::Info, "開始掃描...");

        let detect = match detect_ue_version_with_hint(&game_dir, self.shipping_exe_path.as_deref()) {
            Ok(d) => d,
            Err(e) => {
                self.scan_status = format!("錯誤：{}", e);
                self.push_log(LogLevel::Error, &e);
                self.game_dir = None;
                return;
            }
        };
        self.detected_mode    = detect.mode;
        self.detected_version = detect.version;
        self.selected_version = detect.version;
        self.detect_evidence  = detect.evidence;
        self.push_log(LogLevel::Info, format!("偵測: {}", self.detect_evidence));

        self.do_scan_paks();
    }

    /// 準備 WNE 版本選擇對話框：偵測是否需要 AES
    fn prepare_wne_version_dialog(&mut self, wne_paks: &[PathBuf]) {
        // 嘗試不帶 key 掃描第一個 pak，判斷是否加密
        let test_paks = wne_paks.iter().take(1).cloned().collect::<Vec<_>>();
        let results = scan_all_paks(&test_paks, None);
        self.wne_paks_need_aes = results.iter().any(|(_, r)| r.is_err());
        self.wne_manual_version = None;
        self.wne_aes_input.clear();
        self.show_wne_version_dialog = true;
        self.scan_status = "請手動選擇引擎版本".to_string();
    }

    /// PAK 掃描 + 樹狀建立（detection 完成後共用邏輯）
    fn do_scan_paks(&mut self) {
        let game_dir = self.game_dir.clone().unwrap();

        let paks = find_all_paks(&game_dir);
        self.push_log(LogLevel::Info, format!("找到 {} 個容器檔案 (PAK/UTOC)", paks.len()));

        let key_opt = if self.aes_key.is_empty() { None } else { Some(self.aes_key.as_str()) };
        let scan_results = scan_all_paks(&paks, key_opt);

        let needs_aes = scan_results.iter().any(|(_, r)| r.is_err()) && self.aes_key.is_empty();
        if needs_aes {
            self.scan_status = "需要 AES Key 進行解密".to_string();
            self.show_aes_dialog = true;
            return;
        }

        let mut all_entries: Vec<PakEntry> = vec![];
        for (pak, result) in scan_results {
            let name = pak.file_name().unwrap_or_default().to_string_lossy().into_owned();
            match result {
                Ok(entries) => {
                    self.push_log(LogLevel::Success, format!("  {} -> {} 個條目", name, entries.len()));
                    all_entries.extend(entries);
                }
                Err(e) => {
                    self.push_log(LogLevel::Error, format!("  {} 解析失敗: {}", name, e));
                }
            }
        }

        if self.build_panel.game_name.is_empty() {
            if let Some(name) = all_entries.iter().find_map(|entry| {
                let first = entry.path.split('/').next()?;
                if !first.is_empty() && !first.eq_ignore_ascii_case("engine") {
                    Some(first.to_string())
                } else { None }
            }) {
                self.push_log(LogLevel::Info, format!("自動擷取遊戲名稱: {}", name));
                self.build_panel.game_name = name;
            }
        }

        self.all_paks = paks;
        self.tree_root = build_tree(&all_entries);
        let (dirs, files) = count_entries(&self.tree_root);
        self.scan_status = format!("共 {} 個目錄，{} 個檔案", dirs, files);
        let status = self.scan_status.clone();
        self.push_log(LogLevel::Success, format!("掃描完成：{}", status));
    }
} // end impl App

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.show_multi_exe_warning {
            egui::Window::new("[警告] 目錄選擇錯誤")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.colored_label(Color32::RED, "偵測到多個 Shipping.exe！");
                    ui.label("這通常是因為您選擇了包含多個遊戲的根目錄。");
                    ui.label("請重新選擇「單一遊戲」的正確目錄。");
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("確定").clicked() {
                            self.show_multi_exe_warning = false;
                        }
                    });
                });
        }

        if self.show_aes_dialog {
            egui::Window::new("[鑰匙] 需要 AES Key 解密")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.aes_tab, AesTab::Internal, "內部工具");
                        ui.selectable_value(&mut self.aes_tab, AesTab::External, "外部工具 (手動)");
                    });
                    ui.separator();

                    match self.aes_tab {
                        AesTab::Internal => {
                            ui.label("自動掃描遊戲 Shipping.exe 進行 AES Key 解密。");
                            if let Some(exe) = &self.shipping_exe_path {
                                ui.label(format!("偵測到執行檔: {}", exe.file_name().unwrap_or_default().to_string_lossy()));
                                ui.add_space(10.0);
                                if ui.button("確認並繼續掃描").clicked() {
                                    if let Some(key) = ue_mod_core::aesdumpster::extract_aes_key(exe) {
                                        self.aes_key = format!("0x{}", key);
                                        self.push_log(LogLevel::Success, "成功自動提取 AES Key");
                                        self.show_aes_dialog = false;
                                        self.do_scan();
                                    } else {
                                        self.push_log(LogLevel::Error, "自動提取失敗，請嘗試使用外部工具。");
                                    }
                                }
                            }
                        }
                        AesTab::External => {
                            ui.hyperlink_to("🔗 開啟 AESDumpster", "https://github.com/GHFear/AESDumpster");
                            ui.label("請在連結的 README 中找到 Online Version。");
                            ui.label("點擊網頁中間的 [Drag & Drop your game's (main executable) here] 放入您的執行檔。");
                            ui.label("將 [AES KEY] 中的值填入下方。");
                            ui.add_space(5.0);

                            if let Some(exe) = &self.shipping_exe_path {
                                ui.horizontal(|ui| {
                                    ui.label("您的執行檔路徑: ");
                                    ui.code(exe.display().to_string());
                                    if ui.button("[複製]").clicked() {
                                        ui.output_mut(|o| o.copied_text = exe.display().to_string());
                                    }
                                });
                            }

                            ui.add_space(10.0);
                            ui.horizontal(|ui| {
                                ui.label("AES Key:");
                                ui.text_edit_singleline(&mut self.aes_key);
                            });
                            ui.add_space(5.0);
                            if ui.button("確認並繼續掃描").clicked() {
                                self.show_aes_dialog = false;
                                self.do_scan();
                            }
                        }
                    }

                    ui.separator();
                    if ui.button("取消").clicked() {
                        self.show_aes_dialog = false;
                    }
                });
        }

        // ── EXE 選擇對話框（WNE PAK 找不到 Shipping.exe 時）──────────────────
        if self.show_exe_picker {
            let candidates = self.exe_picker_candidates.clone();
            egui::Window::new("[EXE] 選擇遊戲執行檔")
                .collapsible(false)
                .resizable(true)
                .min_width(520.0)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("偵測到 WindowsNoEditor PAK，但找不到 Shipping.exe。");
                    ui.label("請從下列清單選擇遊戲的執行檔，以嘗試自動偵測引擎版本：");
                    ui.add_space(6.0);

                    ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        for (i, exe_path) in candidates.iter().enumerate() {
                            let display = exe_path.display().to_string();
                            ui.selectable_value(
                                &mut self.exe_picker_selected,
                                Some(i),
                                display,
                            );
                        }
                    });

                    ui.add_space(8.0);
                    ui.separator();
                    ui.horizontal(|ui| {
                        let can_confirm = self.exe_picker_selected.is_some();
                        if ui.add_enabled(can_confirm, egui::Button::new("確認")).clicked() {
                            let idx = self.exe_picker_selected.unwrap();
                            let exe = candidates[idx].clone();
                            self.show_exe_picker = false;

                            if let Some(version) = probe_exe_version(&exe) {
                                // 版本讀取成功 → 直接進入掃描
                                self.shipping_exe_path = Some(exe);
                                self.detected_mode    = BuildMode::Pak;
                                self.detected_version = version;
                                self.selected_version = version;
                                self.detect_evidence  = format!(
                                    "從 EXE ({}) 的 VS_VERSIONINFO 偵測到版本",
                                    candidates[idx].file_name()
                                        .unwrap_or_default().to_string_lossy()
                                );
                                self.push_log(LogLevel::Info, format!("偵測: {}", self.detect_evidence));
                                self.do_scan_paks();
                            } else {
                                // 讀不到版本 → 進入手動版本選擇
                                self.shipping_exe_path = Some(exe);
                                let game_dir = self.game_dir.clone().unwrap_or_default();
                                let wne_paks = find_wne_paks(&game_dir);
                                self.prepare_wne_version_dialog(&wne_paks);
                            }
                        }

                        if ui.button("取消").clicked() {
                            self.show_exe_picker = false;
                            self.game_dir = None;
                            self.scan_status = "已取消選擇執行檔".to_string();
                            self.detect_evidence.clear();
                        }
                    });
                });
        }

        // ── WNE 手動版本選擇對話框（EXE 讀不到版本，或目錄內無 EXE）────────────
        if self.show_wne_version_dialog {
            let mut confirm_clicked = false;
            let mut cancel_clicked  = false;

            egui::Window::new("[設定] 請設定引擎版本")
                .collapsible(false)
                .resizable(false)
                .min_width(460.0)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.colored_label(Color32::YELLOW, "⚠ 無法自動偵測引擎版本。請參考下列說明手動查詢後填入。。");
                    ui.add_space(4.0);
                    ui.hyperlink_to(
                        "🔗 開啟 AESDumpster 查詢引擎版本",
                        "https://github.com/GHFear/AESDumpster",
                    );
                    ui.label("請在連結的 README 中找到 Online Version 並進入。");
                    ui.label("點擊網頁中間的 [Drag & Drop your game's (main executable) here] 放入您的執行檔(或將exe拖曳進去)。");
                    ui.label("從網頁的結果中找Version攔的數值。(EX:4.18)");
                    
                    ui.add_space(8.0);
                    ui.separator();

                    // 版本選擇下拉
                    ui.horizontal(|ui| {
                        ui.label("引擎版本：");
                        let selected_text = match self.wne_manual_version {
                            Some(v) => v.as_str(),
                            None    => "(請選擇)".to_string(),
                        };
                        egui::ComboBox::from_id_source("wne_version_combo")
                            .selected_text(selected_text)
                            .show_ui(ui, |ui| {
                                for &v in UEVersion::all() {
                                    ui.selectable_value(
                                        &mut self.wne_manual_version,
                                        Some(v),
                                        v.as_str(),
                                    );
                                }
                            });
                    });

                    // AES 區塊（僅在需要時顯示）
                    if self.wne_paks_need_aes {
                        ui.add_space(8.0);
                        ui.separator();
                        ui.colored_label(Color32::YELLOW, "⚠ 此 PAK 已加密，需要 AES Key 才能繼續。");
                        ui.hyperlink_to(
                            "🔗 開啟 AESDumpster 查詢 AES Key",
                            "https://github.com/GHFear/AESDumpster",
                        );
                        ui.label("使用方式如上方所示，將結果中的 [AES KEY] 值貼入下方欄位：");
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.label("AES Key：");
                            ui.text_edit_singleline(&mut self.wne_aes_input);
                        });
                    }

                    ui.add_space(10.0);
                    ui.separator();
                    ui.horizontal(|ui| {
                        let aes_ok = !self.wne_paks_need_aes || !self.wne_aes_input.is_empty();
                        let can_confirm = self.wne_manual_version.is_some() && aes_ok;
                        if ui.add_enabled(can_confirm, egui::Button::new("確認並繼續")).clicked() {
                            confirm_clicked = true;
                        }
                        if ui.button("取消").clicked() {
                            cancel_clicked = true;
                        }
                    });
                    if self.wne_manual_version.is_none() {
                        ui.colored_label(Color32::RED, "選擇引擎版本才可繼續。");
                    }
                    if self.wne_paks_need_aes && self.wne_aes_input.is_empty() {
                        ui.colored_label(Color32::RED, "填入 AES Key 才可繼續。");
                    }
                });

            if confirm_clicked {
                let ver = self.wne_manual_version.unwrap();
                self.detected_mode    = BuildMode::Pak;
                self.detected_version = ver;
                self.selected_version = ver;
                self.detect_evidence  = format!("手動設定版本 ({})", ver.as_str());
                if !self.wne_aes_input.is_empty() {
                    self.aes_key = self.wne_aes_input.clone();
                }
                self.push_log(LogLevel::Info, format!("偵測: {}", self.detect_evidence));
                self.show_wne_version_dialog = false;
                self.push_log(LogLevel::Info, "開始掃描...");
                self.do_scan_paks();
            }
            if cancel_clicked {
                self.show_wne_version_dialog = false;
                self.game_dir = None;
                self.scan_status = "已取消版本設定".to_string();
                self.detect_evidence.clear();
            }
        }

        TopBottomPanel::top("toolbar").show(ctx, |ui| self.show_toolbar(ui));
        TopBottomPanel::bottom("statusbar").show(ctx, |ui| self.show_statusbar(ui));

        if self.active_panel == ActivePanel::About {
            CentralPanel::default().show(ctx, |ui| self.show_main_panel(ui));
            return;
        }

        let total_width = ctx.screen_rect().width();
        let left_width = total_width * 0.35;
        let right_width = total_width * 0.20;

        let hide_right_panel =
            self.active_panel == ActivePanel::LocresEditor && self.locres_editor.is_loaded;

        SidePanel::left("left_panel")
            .exact_width(left_width)
            .resizable(false)
            .show(ctx, |ui| self.show_left_panel(ui));

        if !hide_right_panel {
            SidePanel::right("staging_panel")
                .exact_width(right_width)
                .resizable(false)
                .show(ctx, |ui| self.show_staging_panel(ui));
        }

        CentralPanel::default().show(ctx, |ui| self.show_main_panel(ui));
    }
}

impl App {
    fn show_toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.heading("Unreal Engine L10n Mod Tool");
            ui.separator();

            for (panel, label) in [
                (ActivePanel::Files,        "檔案"),
                (ActivePanel::LocresEditor, "在地化編輯"),
                (ActivePanel::Build,        "建構"),
                (ActivePanel::Log,          "日誌"),
                (ActivePanel::About,        "關於"),
            ] {
                if ui.selectable_label(self.active_panel == panel, label).clicked() {
                    self.active_panel = panel;
                }
            }

            ui.separator();

            let changes = self.staging.total_changes();
            if changes > 0 {
                ui.colored_label(Color32::from_rgb(255, 200, 50), format!("{} 個修改待建構", changes));
            } else {
                ui.label(RichText::new("無修改").color(Color32::GRAY));
            }
        });
    }

    fn show_statusbar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if !self.detect_evidence.is_empty() {
                let mode_str = match self.detected_mode {
                    BuildMode::IoStore => "IoStore",
                    BuildMode::Pak => "PAK",
                };
                ui.label(format!("{} | {} | {}", mode_str, self.detected_version, self.detect_evidence));
            } else {
                ui.label(&self.scan_status);
            }
        });
    }

    fn show_left_panel(&mut self, ui: &mut Ui) {
        CollapsingHeader::new("遊戲目錄").default_open(true).show(ui, |ui| {
            self.show_dir_settings(ui);
        });

        ui.separator();
        ui.label("檔案瀏覽");
        ui.separator();

        ScrollArea::both().show(ui, |ui| {
            let tree = std::mem::take(&mut self.tree_root);
            let mut open_locres: Option<(String, PathBuf)> = None;
            let mut add_font: Option<FontReplacement> = None;
            let mut batch_fonts: Option<Vec<FontReplacement>> = None;

            crate::tree_view::show_tree(ui, &tree, &mut self.tree_view, &mut open_locres, &mut add_font, &mut batch_fonts);
            self.tree_root = tree;

            if let Some((path, pak)) = open_locres {
                self.locres_editor.open_locres(&path, &pak, self.aes_key.as_str());
                self.active_panel = ActivePanel::LocresEditor;
            }
            if let Some(font) = add_font {
                // 若相同路徑已存在，直接覆蓋；否則新增
                if let Some(existing) = self.staging.font_replacements.iter_mut()
                    .find(|f| f.pak_path == font.pak_path)
                {
                    *existing = font;
                } else {
                    self.staging.font_replacements.push(font);
                }
                self.push_log(LogLevel::Info, "已加入字體替換");
            }
            if let Some(fonts) = batch_fonts {
                let count = fonts.len();
                // 批量字體亦以覆蓋邏輯處理，確保不重複
                for font in fonts {
                    if let Some(existing) = self.staging.font_replacements.iter_mut()
                        .find(|f| f.pak_path == font.pak_path)
                    {
                        *existing = font;
                    } else {
                        self.staging.font_replacements.push(font);
                    }
                }
                self.push_log(LogLevel::Info, format!("已批量加入 {} 個字體替換", count));
                self.tree_view.multi_selected.clear();
            }
        });
    }

    fn show_dir_settings(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let label = self.game_dir.as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "請選擇遊戲目錄".to_string());

            if ui.add(Button::new("瀏覽").fill(Color32::from_rgb(50, 120, 50))).clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    self.staging = StagingArea::default();
                    self.locres_editor = LocresEditorState::default();
                    self.build_panel = BuildPanelState::default();
                    self.tree_root.clear();
                    self.all_paks.clear();
                    self.log.clear();
                    self.aes_key.clear();
                    self.scan_status = "尚未掃描".to_string();
                    // 清除上一次掃描的偵測結果，確保底部狀態列不殘留舊資訊
                    self.detect_evidence.clear();
                    // 清除 WNE 流程狀態
                    self.show_exe_picker = false;
                    self.exe_picker_candidates.clear();
                    self.exe_picker_selected = None;
                    self.show_wne_version_dialog = false;
                    self.wne_manual_version = None;
                    self.wne_aes_input.clear();
                    self.game_dir = Some(path);
                    self.do_scan();
                }
            }

            ScrollArea::horizontal()
                .id_source("game_dir_scroll")
                .show(ui, |ui| {
                    ui.label(label)
                        .on_hover_text("遊戲根目錄（如:SteamLibrary/steamapps/common/遊戲名）");
                });
        });

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let can_scan = self.game_dir.is_some();
            if ui.add_enabled(can_scan, Button::new("重新掃描資源")).clicked() {
                self.do_scan();
            }
            if !can_scan {
                ui.label(RichText::new("請先設定遊戲目錄").color(Color32::GRAY).small());
            }
        });
    }

    fn show_staging_panel(&mut self, ui: &mut Ui) {
        ui.heading("待建構區");
        ui.separator();

        if self.staging.is_empty() {
            ui.colored_label(Color32::GRAY, "尚無修改");
            return;
        }

        ScrollArea::vertical().show(ui, |ui| {
            if !self.staging.locres_edits.is_empty() {
                ui.label("在地化");
                for (path, entries) in &self.staging.locres_edits {
                    let count = entries.iter().filter(|e| e.is_modified()).count();
                    let name = path.rsplit('/').next().unwrap_or(path);
                    ui.horizontal(|ui| {
                        ui.label(format!("  {} ({})", name, count));
                    });
                }
                ui.separator();
            }

            if !self.staging.font_replacements.is_empty() {
                ui.label("字體替換");
                for fr in &self.staging.font_replacements {
                    let name = fr.pak_path.rsplit('/').next().unwrap_or(&fr.pak_path);
                    ui.label(format!("  {}", name));
                }
                ui.separator();
            }

            ui.add_space(4.0);
            if ui.button("清除所有").clicked() {
                self.staging = StagingArea::default();
            }
        });
    }

    fn show_main_panel(&mut self, ui: &mut Ui) {
        match self.active_panel {
            ActivePanel::Files => {
                ui.heading("檔案詳細資訊");
                ui.separator();

                let multi_fonts: Vec<String> = self.tree_view.multi_selected.iter()
                    .filter(|p| p.ends_with(".ufont") || p.ends_with(".ttf") || p.ends_with(".otf"))
                    .cloned()
                    .collect();

                if !multi_fonts.is_empty() {
                    ui.horizontal(|ui| {
                        ui.colored_label(Color32::from_rgb(100, 220, 100), format!("已選擇 {} 個字型檔案", multi_fonts.len()));
                        if ui.button("批量替換所選字體").clicked() {
                            if let Some(new_font) = rfd::FileDialog::new()
                                .add_filter("字體檔案", &["ttf", "otf", "ufont"])
                                .pick_file()
                            {
                                for font_path in multi_fonts {
                                    let font = FontReplacement {
                                        pak_path: font_path,
                                        replacement: new_font.clone(),
                                    };
                                    // 相同路徑已存在則覆蓋，否則新增
                                    if let Some(existing) = self.staging.font_replacements.iter_mut()
                                        .find(|f| f.pak_path == font.pak_path)
                                    {
                                        *existing = font;
                                    } else {
                                        self.staging.font_replacements.push(font);
                                    }
                                }
                                self.push_log(LogLevel::Info, "已加入批量字體替換");
                                self.tree_view.multi_selected.clear();
                            }
                        }
                    });
                    ui.separator();
                }

                ui.heading("可能需要修改的資源 (在地化與字體)");

                let mut important_entries = vec![];
                fn collect_important(nodes: &[TreeNode], out: &mut Vec<PakEntry>) {
                    for node in nodes {
                        match node {
                            TreeNode::File(e) if e.is_locres() || e.is_font() => out.push(e.clone()),
                            TreeNode::Dir { children, .. } => collect_important(children, out),
                            _ => {}
                        }
                    }
                }
                collect_important(&self.tree_root, &mut important_entries);

                if important_entries.is_empty() {
                    ui.label("尚無資源或尚未掃描。");
                } else {
                    let mut grouped: BTreeMap<String, (Vec<PakEntry>, Vec<PakEntry>)> = BTreeMap::new();
                    for entry in important_entries {
                        let pak_name = entry.pak.file_name().unwrap_or_default().to_string_lossy().into_owned();
                        let group = grouped.entry(pak_name).or_insert_with(|| (vec![], vec![]));
                        if entry.is_locres() { group.0.push(entry); }
                        else if entry.is_font() { group.1.push(entry); }
                    }

                    ScrollArea::both().show(ui, |ui| {
                        for (pak_name, (locres_list, font_list)) in grouped {
                            ui.heading(RichText::new(&pak_name).color(Color32::from_rgb(180, 220, 255)));

                            if !locres_list.is_empty() {
                                ui.label(RichText::new("在地化 (Locres)").strong().color(Color32::from_rgb(150, 230, 150)));
                                for entry in locres_list {
                                    ui.horizontal(|ui| {
                                        if ui.button("編輯").clicked() {
                                            self.locres_editor.open_locres(&entry.path, &entry.pak, self.aes_key.as_str());
                                            self.active_panel = ActivePanel::LocresEditor;
                                        }
                                        ui.label(RichText::new(entry.file_name()).strong());
                                        ui.label(RichText::new(&entry.path).small().color(Color32::DARK_GRAY));
                                    });
                                }
                                ui.add_space(4.0);
                            }

                            if !font_list.is_empty() {
                                ui.label(RichText::new("字體 (Fonts)").strong().color(Color32::from_rgb(230, 180, 100)));
                                for entry in font_list {
                                    ui.horizontal(|ui| {
                                        if ui.button("替換").clicked() {
                                            if let Some(new_font) = rfd::FileDialog::new()
                                                .add_filter("字體檔案", &["ttf", "otf", "ufont"])
                                                .pick_file()
                                            {
                                                let font = FontReplacement {
                                                    pak_path: entry.path.clone(),
                                                    replacement: new_font,
                                                };
                                                // 相同路徑已存在則覆蓋，否則新增
                                                if let Some(existing) = self.staging.font_replacements.iter_mut()
                                                    .find(|f| f.pak_path == font.pak_path)
                                                {
                                                    *existing = font;
                                                } else {
                                                    self.staging.font_replacements.push(font);
                                                }
                                                self.push_log(LogLevel::Info, "已加入字體替換");
                                            }
                                        }
                                        ui.label(RichText::new(entry.file_name()).strong());
                                        ui.label(RichText::new(&entry.path).small().color(Color32::DARK_GRAY));
                                    });
                                }
                                ui.add_space(4.0);
                            }
                            ui.separator();
                        }
                    });
                }
            }
            ActivePanel::LocresEditor => {
                let staging = &mut self.staging;
                crate::locres_editor::show_locres_editor(ui, &mut self.locres_editor, staging);
            }
            ActivePanel::Build => {
                crate::build_panel::show_build_panel(
                    ui, &mut self.build_panel, &mut self.staging, &mut self.mod_name,
                    &mut self.output_dir, &mut self.detected_mode, &mut self.detected_version,
                    &mut self.manual_version_override, &mut self.selected_version,
                    &self.all_paks, &mut self.log, &self.aes_key,
                );
            }
            ActivePanel::Log => {
                show_log_panel(ui, &self.log);
            }
            ActivePanel::About => {
                ui.heading("關於 (About)");
                ui.separator();

                ui.label(RichText::new("工具整合開發 (Tool Integration)").strong());
                ui.horizontal(|ui| {
                    ui.label("by: Tents89");
                    ui.label(" | Releases: ");
                    ui.hyperlink_to("tents89/UEGame-SC2TC", "https://github.com/tents89/UEGame-SC2TC");
                });

                ui.add_space(15.0);
                ui.label(RichText::new("Credits：").strong());
                ui.add_space(5.0);

                const LIBS: &[(&str, &str, &str, &str)] = &[
                    ("repak",        "trumank",   "https://github.com/trumank/repak",          "Apache-2.0, MIT licenses"),
                    ("retoc",        "trumank",   "https://github.com/trumank/retoc",          "MIT licenses"),
                    ("locres-rs",    "AceHanded", "https://github.com/AceHanded/locres-rs",   "Apache-2.0 license"),
                    ("opencc-rust",  "doggy8088", "https://github.com/doggy8088/opencc-rust", "MIT license"),
                    ("aesdumpster-rs", "yuhkix", "https://github.com/yuhkix/aesdumpster-rs", "Unknown license"),
                    ("繁化姬", "zhconvert", "https://zhconvert.org", "請參考頁面"),
                ];

                for (name, _author, link, license) in LIBS {
                    ui.horizontal(|ui| {
                        ui.label(format!("- {}: ", name));
                        ui.hyperlink_to(*link, *link);
                        ui.label(format!("({})", license));
                    });
                }

                ui.add_space(10.0);
                ui.label(
                    RichText::new("If you prefer your code not to be used, please let me know and I will remove it.")
                        .italics()
                        .color(Color32::GRAY),
                );
            }
        }
    }
}

fn show_log_panel(ui: &mut Ui, log: &[LogEntry]) {
    ui.heading("操作日誌");
    ui.separator();
    ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
        for entry in log {
            let color = match entry.level {
                LogLevel::Info    => Color32::from_gray(200),
                LogLevel::Success => Color32::from_rgb(100, 220, 100),
                LogLevel::Warning => Color32::from_rgb(255, 200, 50),
                LogLevel::Error   => Color32::from_rgb(255, 80, 80),
            };
            ui.horizontal(|ui| {
                ui.colored_label(Color32::GRAY, format!("[{}]", entry.level.label()));
                ui.colored_label(color, &entry.message);
            });
        }
    });
}
