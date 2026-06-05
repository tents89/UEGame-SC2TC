use std::collections::BTreeMap;
use std::path::PathBuf;

use egui::*;
use ue_mod_core::*;

use crate::tree_view::TreeViewState;
use crate::locres_editor::LocresEditorState;
use crate::build_panel::BuildPanelState;
use crate::dev_mode::{ExternalFilter, FilterCache};

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

    // 開發者模式
    pub settings: Settings,
    pub show_dev_warning: bool,
    pub dev_skip_warning_temp: bool,
    pub dev_browser_path: Vec<String>,
    pub dev_external_filter: Option<ExternalFilter>,
    pub dev_external_filter_name: Option<String>,
    pub filter_cache: FilterCache,
    pub tree_file_count: usize,
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
            settings: Settings::default(),
            show_dev_warning: false,
            dev_skip_warning_temp: false,
            dev_browser_path: vec![],
            dev_external_filter: None,
            dev_external_filter_name: None,
            filter_cache: FilterCache::default(),
            tree_file_count: 0,
        }
    }
}

impl App {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let settings = Settings::load_or_create();
        let show_dev_warning = settings.is_dev_mode() && !settings.skip_warning;
        Self {
            settings,
            show_dev_warning,
            ..Self::default()
        }
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

        // 搜尋 Shipping EXE（大小寫不敏感，相容 Linux/macOS 上開啟 Windows 版檔案的情境）
        let mut shipping_exes = vec![];
        for entry in walkdir::WalkDir::new(&game_dir).max_depth(5).into_iter().flatten() {
            let p = entry.path();
            if !p.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("exe")) { continue; }
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
            let path_lower = p.to_string_lossy().to_lowercase();
            if path_lower.contains("binaries") && name.contains("shipping") {
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
        let dev_mode = self.settings.is_dev_mode();
        let scan_results = if dev_mode {
            scan_all_paks_full(&paks, key_opt)
        } else {
            scan_all_paks(&paks, key_opt)
        };

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
        self.tree_file_count = files;
        self.scan_status = format!("共 {} 個目錄，{} 個檔案", dirs, files);
        let status = self.scan_status.clone();
        self.push_log(LogLevel::Success, format!("掃描完成：{}", status));
    }
} // end impl App

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.show_dev_warning {
            let mut close = false;
            let mut skip = self.dev_skip_warning_temp;
            egui::Window::new("[進階] 開發者模式")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.colored_label(Color32::from_rgb(255, 200, 50), "目前為進階模式");
                    ui.label("可以解包檔案與探索資產目錄，不提供製作模組功能。");
                    ui.label("若不需要，請在 settings.json 將 dev_mode 改為 0 關閉。");
                    ui.add_space(8.0);
                    ui.checkbox(&mut skip, "不再提醒");
                    ui.add_space(6.0);
                    if ui.button("我了解").clicked() {
                        close = true;
                    }
                });
            self.dev_skip_warning_temp = skip;
            if close {
                if skip {
                    self.settings.skip_warning = true;
                    if let Err(e) = self.settings.save() {
                        self.push_log(LogLevel::Warning, format!("無法寫入 settings.json: {}", e));
                    }
                }
                self.show_dev_warning = false;
            }
        }

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

        let dev_mode = self.settings.is_dev_mode();

        // 每幀於 UI 繪製前檢查並重建 cache（指紋一致則零成本跳過）
        self.filter_cache.rebuild_if_needed(
            &self.tree_root,
            &self.tree_view.filter,
            self.tree_view.use_regex,
            self.dev_external_filter.as_ref(),
            self.tree_file_count,
        );

        // dev_mode=1 強制將被隱藏的面板切回 Files
        if dev_mode && matches!(self.active_panel, ActivePanel::LocresEditor | ActivePanel::Build) {
            self.active_panel = ActivePanel::Files;
        }

        if self.active_panel == ActivePanel::About {
            CentralPanel::default().show(ctx, |ui| self.show_main_panel(ui));
            return;
        }

        let total_width = ctx.screen_rect().width();
        let left_width = total_width * 0.35;
        let right_width = total_width * 0.20;

        let hide_right_panel = dev_mode
            || (self.active_panel == ActivePanel::LocresEditor && self.locres_editor.is_loaded);

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

            let dev_mode = self.settings.is_dev_mode();
            let panels: &[(ActivePanel, &str)] = if dev_mode {
                &[
                    (ActivePanel::Files, "檔案"),
                    (ActivePanel::Log,   "日誌"),
                    (ActivePanel::About, "關於"),
                ]
            } else {
                &[
                    (ActivePanel::Files,        "檔案"),
                    (ActivePanel::LocresEditor, "在地化編輯"),
                    (ActivePanel::Build,        "建構"),
                    (ActivePanel::Log,          "日誌"),
                    (ActivePanel::About,        "關於"),
                ]
            };

            for (panel, label) in panels {
                if ui.selectable_label(self.active_panel == *panel, *label).clicked() {
                    self.active_panel = *panel;
                }
            }

            ui.separator();

            if dev_mode {
                ui.colored_label(Color32::from_rgb(255, 200, 50), "進階模式 (dev_mode = 1)");
            } else {
                let changes = self.staging.total_changes();
                if changes > 0 {
                    ui.colored_label(Color32::from_rgb(255, 200, 50), format!("{} 個修改待建構", changes));
                } else {
                    ui.label(RichText::new("無修改").color(Color32::GRAY));
                }
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
            let mut batch_fonts: Option<(PathBuf, Vec<String>)> = None;
            let mut navigate_to: Option<Vec<String>> = None;

            crate::tree_view::show_tree(
                ui, &tree, &mut self.tree_view,
                &mut open_locres, &mut add_font, &mut batch_fonts,
                &self.filter_cache,
                &mut navigate_to,
            );
            self.tree_root = tree;

            if let Some(target) = navigate_to {
                // 開發者模式下，點左樹資料夾或檔案 → 把右側「當下目錄」同步過去。
                if self.settings.is_dev_mode() {
                    self.dev_browser_path = target;
                }
            }

            if let Some((path, pak)) = open_locres {
                self.locres_editor.open_locres(&path, &pak, self.aes_key.as_str());
                self.active_panel = ActivePanel::LocresEditor;
            }
            if let Some(font) = add_font {
                // 以 (source_pak, pak_path) 去重；相同則覆蓋。
                if let Some(existing) = self.staging.font_replacements.iter_mut()
                    .find(|f| f.source_pak == font.source_pak && f.pak_path == font.pak_path)
                {
                    *existing = font;
                } else {
                    self.staging.font_replacements.push(font);
                }
                self.push_log(LogLevel::Info, "已加入字體替換");
            }
            if let Some((replacement, paths)) = batch_fonts {
                // 把內部路徑展開成所有來源 pak 的 FontReplacement。
                let mut all_files: Vec<PakEntry> = vec![];
                fn collect(nodes: &[TreeNode], out: &mut Vec<PakEntry>) {
                    for n in nodes {
                        match n {
                            TreeNode::Dir { children, .. } => collect(children, out),
                            TreeNode::File(e) => out.push(e.clone()),
                        }
                    }
                }
                collect(&self.tree_root, &mut all_files);

                let mut added = 0usize;
                for p in &paths {
                    for entry in all_files.iter().filter(|e| &e.path == p) {
                        let font = FontReplacement {
                            source_pak: entry.pak.clone(),
                            pak_path: p.clone(),
                            replacement: replacement.clone(),
                        };
                        if let Some(existing) = self.staging.font_replacements.iter_mut()
                            .find(|f| f.source_pak == font.source_pak && f.pak_path == font.pak_path)
                        {
                            *existing = font;
                        } else {
                            self.staging.font_replacements.push(font);
                            added += 1;
                        }
                    }
                }
                self.push_log(LogLevel::Info, format!("已批量加入 {} 個字體替換", added));
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
                for ((source_pak, path), entries) in &self.staging.locres_edits {
                    let count = entries.iter().filter(|e| e.is_modified()).count();
                    let name = path.rsplit('/').next().unwrap_or(path);
                    let pak_name = source_pak.file_name().unwrap_or_default().to_string_lossy();
                    ui.horizontal(|ui| {
                        ui.label(format!("  {} ({})", name, count))
                            .on_hover_text(format!("{}\n← {}", path, pak_name));
                    });
                }
                ui.separator();
            }

            if !self.staging.font_replacements.is_empty() {
                ui.label("字體替換");
                for fr in &self.staging.font_replacements {
                    let name = fr.pak_path.rsplit('/').next().unwrap_or(&fr.pak_path);
                    let pak_name = fr.source_pak.file_name().unwrap_or_default().to_string_lossy();
                    ui.label(format!("  {} ← {}", name, pak_name))
                        .on_hover_text(format!("{}\n← {}", fr.pak_path, pak_name));
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
                if self.settings.is_dev_mode() {
                    self.show_dev_browser(ui);
                    return;
                }
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
                                // 為每個選到的內部路徑找出所有來源 pak（同名跨 pak 都加入）。
                                let mut all_files: Vec<PakEntry> = vec![];
                                fn collect(nodes: &[TreeNode], out: &mut Vec<PakEntry>) {
                                    for n in nodes {
                                        match n {
                                            TreeNode::Dir { children, .. } => collect(children, out),
                                            TreeNode::File(e) => out.push(e.clone()),
                                        }
                                    }
                                }
                                collect(&self.tree_root, &mut all_files);

                                let mut added = 0usize;
                                for font_path in &multi_fonts {
                                    for entry in all_files.iter().filter(|e| &e.path == font_path) {
                                        let font = FontReplacement {
                                            source_pak: entry.pak.clone(),
                                            pak_path: font_path.clone(),
                                            replacement: new_font.clone(),
                                        };
                                        // 以 (source_pak, pak_path) 去重；同 pak 重複則覆蓋。
                                        if let Some(existing) = self.staging.font_replacements.iter_mut()
                                            .find(|f| f.source_pak == font.source_pak && f.pak_path == font.pak_path)
                                        {
                                            *existing = font;
                                        } else {
                                            self.staging.font_replacements.push(font);
                                            added += 1;
                                        }
                                    }
                                }
                                self.push_log(LogLevel::Info, format!("已加入批量字體替換 {} 個", added));
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
                                                    source_pak: entry.pak.clone(),
                                                    pak_path: entry.path.clone(),
                                                    replacement: new_font,
                                                };
                                                // 同 (source_pak, pak_path) 已存在則覆蓋，否則新增
                                                if let Some(existing) = self.staging.font_replacements.iter_mut()
                                                    .find(|f| f.source_pak == font.source_pak && f.pak_path == font.pak_path)
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
                ui.label(RichText::new("進階模式").strong());
                let dev_mode_now = self.settings.is_dev_mode();
                let mut dev_mode_new = dev_mode_now;
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut dev_mode_new, "啟用進階模式（解包/穿梭瀏覽，不提供模組製作）").changed() {
                        self.settings.dev_mode = if dev_mode_new { 1 } else { 0 };
                        match self.settings.save() {
                            Ok(()) => {
                                self.push_log(
                                    LogLevel::Success,
                                    format!(
                                        "已將 dev_mode 改為 {}，請重新啟動工具讓設定生效。",
                                        self.settings.dev_mode
                                    ),
                                );
                            }
                            Err(e) => {
                                self.push_log(
                                    LogLevel::Error,
                                    format!("寫入 settings.json 失敗: {}", e),
                                );
                            }
                        }
                    }
                });
                ui.colored_label(
                    Color32::from_rgb(255, 200, 50),
                    "切換後需重新啟動本工具才會套用，以避免兩種模式的 UI 與狀態互相衝突。",
                );

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

    // ── 開發者模式：資料夾穿梭瀏覽器 ────────────────────────────────────────
    fn show_dev_browser(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.heading("資料夾穿梭模式");
            ui.separator();
            ui.colored_label(Color32::from_rgb(255, 200, 50), "進階模式");
        });

        // 工具列：上一頁、外部 JSON 導入、批量導出
        ui.horizontal(|ui| {
            let can_back = !self.dev_browser_path.is_empty();
            if ui.add_enabled(can_back, Button::new("◀ 上一頁")).clicked() {
                self.dev_browser_path.pop();
            }
            if ui.button("⌂ 根目錄").clicked() {
                self.dev_browser_path.clear();
            }
            ui.separator();
            if ui.button("導入外部 JSON 篩選").clicked() {
                if let Some(file) = rfd::FileDialog::new()
                    .add_filter("JSON", &["json"])
                    .pick_file()
                {
                    match crate::dev_mode::load_json_path_filter(&file) {
                        Ok(set) => {
                            let n = set.len();
                            let ext = ExternalFilter::from_paths(set);
                            self.dev_external_filter = Some(ext);
                            self.dev_external_filter_name = Some(
                                file.file_name().unwrap_or_default().to_string_lossy().into_owned()
                            );
                            self.push_log(LogLevel::Success, format!("已導入 JSON 篩選：{} 個 path", n));

                            // JSON 載入後立刻重建 cache 以獲得 matched_files
                            self.filter_cache.rebuild_if_needed(
                                &self.tree_root,
                                &self.tree_view.filter,
                                self.tree_view.use_regex,
                                self.dev_external_filter.as_ref(),
                                self.tree_file_count,
                            );

                            // 自動把所有匹配的檔案選取起來，方便批量導出
                            let matched = self.filter_cache.matched_files.clone();
                            self.tree_view.multi_selected = matched;
                            self.tree_view.selected_path = None;
                            self.push_log(
                                LogLevel::Info,
                                format!("自動選取 {} 個匹配檔案", self.tree_view.multi_selected.len()),
                            );
                        }
                        Err(e) => {
                            self.push_log(LogLevel::Error, format!("JSON 解析失敗: {}", e));
                        }
                    }
                }
            }
            if self.dev_external_filter.is_some() {
                if ui.button("清除 JSON 篩選").clicked() {
                    self.dev_external_filter = None;
                    self.dev_external_filter_name = None;
                }
            }

            ui.separator();
            let sel_count = self.tree_view.multi_selected.len();
            ui.label(format!("已選取 {}", sel_count));
            if sel_count > 0 && ui.button("清除選取").clicked() {
                self.tree_view.multi_selected.clear();
                self.tree_view.selected_path = None;
            }
            let can_export = sel_count > 0;
            if ui.add_enabled(can_export, Button::new("批量導出選取項目"))
                .on_hover_text("把所選的檔案從 PAK / IoStore 原樣寫到指定資料夾。")
                .clicked()
            {
                self.export_selected_entries();
            }
        });

        if let Some(name) = &self.dev_external_filter_name {
            ui.colored_label(
                Color32::from_rgb(150, 200, 255),
                format!(
                    "[JSON 篩選] {}（{} 條 path / 匹配 {} 個檔案）",
                    name,
                    self.dev_external_filter.as_ref().map(|e| e.len()).unwrap_or(0),
                    self.filter_cache.total_matches,
                ),
            );
        }

        // 即時路徑顯示
        ui.horizontal(|ui| {
            ui.label(RichText::new("當下目錄：").strong());
            if self.dev_browser_path.is_empty() {
                ui.label(RichText::new("/").color(Color32::LIGHT_BLUE));
            } else {
                let mut click_to: Option<usize> = None;
                ui.label("/");
                for (i, seg) in self.dev_browser_path.iter().enumerate() {
                    if ui.link(seg).clicked() {
                        click_to = Some(i + 1);
                    }
                    ui.label("/");
                }
                if let Some(depth) = click_to {
                    self.dev_browser_path.truncate(depth);
                }
            }
        });

        ui.separator();

        // 搜尋（支援 Regex）
        ui.horizontal(|ui| {
            ui.label("搜尋:");
            ui.text_edit_singleline(&mut self.tree_view.filter);
            if ui.small_button("X").clicked() {
                self.tree_view.filter.clear();
            }
            ui.checkbox(&mut self.tree_view.use_regex, "Regex");

            if self.tree_view.use_regex && !self.tree_view.filter.is_empty() {
                if regex::RegexBuilder::new(&self.tree_view.filter).case_insensitive(true).build().is_err() {
                    ui.colored_label(Color32::RED, "Regex 語法錯誤");
                }
            }
        });

        ui.separator();

        // 解析當下目錄的子節點
        let path = self.dev_browser_path.clone();
        let nodes_at = match find_nodes_at_path(&self.tree_root, &path) {
            Some(n) => n,
            None => {
                ui.colored_label(Color32::RED, "目錄不存在（可能已被移除），已退回上一層。");
                if !self.dev_browser_path.is_empty() {
                    self.dev_browser_path.pop();
                }
                return;
            }
        };

        // 「當下目錄全選」工具列。把對 self.tree_view 的可變寫入延後到借用 nodes_at 之外。
        let mut to_insert: Vec<String> = Vec::new();
        ui.horizontal(|ui| {
            if ui.button("選取本層全部").clicked() {
                let cache_active = self.filter_cache.is_active();
                for node in nodes_at {
                    if let TreeNode::File(e) = node {
                        if !cache_active || self.filter_cache.matched_files.contains(&e.path) {
                            to_insert.push(e.path.clone());
                        }
                    }
                }
            }
            if ui.button("選取本層遞迴全部").clicked() {
                let cache_active = self.filter_cache.is_active();
                let mut files: Vec<&PakEntry> = Vec::new();
                crate::dev_mode::collect_all_files(nodes_at, &mut files);
                for e in files {
                    if !cache_active || self.filter_cache.matched_files.contains(&e.path) {
                        to_insert.push(e.path.clone());
                    }
                }
            }
        });

        ui.separator();

        let cache_active = self.filter_cache.is_active();
        let mut enter_dir: Option<String> = None;
        let mut toggle_select: Option<String> = None;

        ScrollArea::both().show(ui, |ui| {
            let mut shown = 0usize;
            for node in nodes_at {
                match node {
                    TreeNode::Dir { name, children: _, .. } => {
                        let dir_full = build_full_path_for(&path, name);
                        if cache_active && !self.filter_cache.matched_dirs.contains(&dir_full) {
                            continue;
                        }
                        shown += 1;
                        let label = RichText::new(format!("📁 {}", name))
                            .color(Color32::from_rgb(180, 200, 255));
                        if ui.selectable_label(false, label).clicked() {
                            enter_dir = Some(name.clone());
                        }
                    }
                    TreeNode::File(entry) => {
                        if cache_active && !self.filter_cache.matched_files.contains(&entry.path) {
                            continue;
                        }
                        shown += 1;
                        let icon = if entry.is_locres() { "📝" }
                            else if entry.is_font() { "🔤" }
                            else { "📄" };
                        let file_name = entry.file_name();
                        let mut label = RichText::new(format!("{} {}", icon, file_name));
                        if entry.is_locres() {
                            label = label.color(Color32::from_rgb(150, 230, 150));
                        } else if entry.is_font() {
                            label = label.color(Color32::from_rgb(230, 180, 100));
                        }
                        let selected = self.tree_view.multi_selected.contains(&entry.path);
                        let resp = ui.selectable_label(selected, label);
                        if resp.clicked() {
                            toggle_select = Some(entry.path.clone());
                        }
                        let pak_name = entry.pak.file_name().unwrap_or_default().to_string_lossy();
                        let entry_path = entry.path.clone();
                        let entry_pak = entry.pak.clone();
                        let pak_display = entry.pak.display().to_string();
                        resp.on_hover_text(format!("{}\n← {}", entry_path, pak_name)).context_menu(|ui| {
                            ui.label(RichText::new(&entry_path).small().color(Color32::GRAY));
                            ui.separator();
                            if ui.button("複製內部路徑").clicked() {
                                ui.output_mut(|o| o.copied_text = entry_path.clone());
                                ui.close_menu();
                            }
                            if ui.button("複製 PAK 路徑").clicked() {
                                ui.output_mut(|o| o.copied_text = pak_display);
                                ui.close_menu();
                            }
                            let _ = entry_pak; // 保留供未來右鍵 → 單檔導出使用
                        });
                    }
                }
            }
            if shown == 0 {
                ui.colored_label(Color32::GRAY, "（無項目）");
            }
        });

        if let Some(name) = enter_dir {
            self.dev_browser_path.push(name);
        }
        if let Some(p) = toggle_select {
            // dev mode 下點擊即 toggle 多選（保留 selected_path 顯示）
            self.tree_view.select(p, true);
        }
        for p in to_insert {
            self.tree_view.multi_selected.insert(p);
        }
    }

    // ── 批量導出 ───────────────────────────────────────────────────────────
    fn export_selected_entries(&mut self) {
        // 先把選取項複製出來，免得後面開檔對話框、log 寫入時跟 self 借用衝突。
        let selected: Vec<String> = self.tree_view.multi_selected.iter().cloned().collect();
        if selected.is_empty() {
            self.push_log(LogLevel::Warning, "尚未選取任何項目，已取消導出。");
            return;
        }

        let target = match rfd::FileDialog::new().pick_folder() {
            Some(p) => p,
            None => {
                self.push_log(LogLevel::Info, "已取消選擇導出目錄。");
                return;
            }
        };

        self.push_log(
            LogLevel::Info,
            format!("批量導出開始：{} 個項目 → {}", selected.len(), target.display()),
        );

        // 把 multi_selected 路徑 → pak_path 索引（owned，避免後續借用衝突）
        let path_to_pak: std::collections::HashMap<String, PathBuf> = {
            let mut all_files: Vec<&PakEntry> = Vec::new();
            crate::dev_mode::collect_all_files(&self.tree_root, &mut all_files);
            all_files.iter().map(|e| (e.path.clone(), e.pak.clone())).collect()
        };

        let aes_opt = if self.aes_key.is_empty() { None } else { Some(self.aes_key.clone()) };

        // 依 pak 分組 → 同一容器內共用一次 IoStore open / FZenPackageContext。
        let mut grouped: std::collections::BTreeMap<PathBuf, Vec<String>> =
            std::collections::BTreeMap::new();
        let mut missing: Vec<String> = Vec::new();
        for ip in &selected {
            match path_to_pak.get(ip) {
                Some(pak) => grouped.entry(pak.clone()).or_default().push(ip.clone()),
                None => missing.push(ip.clone()),
            }
        }

        let mut ok = 0usize;
        let mut fail = 0usize;
        let mut written_files = 0usize;
        let mut errs: Vec<String> = Vec::new();

        for ip in missing {
            fail += 1;
            errs.push(format!("找不到對應 PakEntry: {}", ip));
        }

        for (pak_path, paths) in grouped {
            let results = ue_mod_core::extract::export_entries_to_dir(
                &pak_path,
                &paths,
                &target,
                aes_opt.as_deref(),
            );
            for (ip, res) in results {
                match res {
                    Ok(files) => {
                        ok += 1;
                        written_files += files.len();
                    }
                    Err(e) => {
                        fail += 1;
                        errs.push(format!("導出失敗 {}: {:#}", ip, e));
                    }
                }
            }
        }

        for msg in errs {
            self.push_log(LogLevel::Error, msg);
        }
        self.push_log(
            LogLevel::Success,
            format!(
                "批量導出完成：項目成功 {}，失敗 {}，實際寫出 {} 個檔案（→ {}）",
                ok, fail, written_files, target.display()
            ),
        );
    }
}

// ── 開發者模式輔助函式 ─────────────────────────────────────────────────────

/// 從根節點開始，按 path segments 逐層往下尋找該目錄的 children。
fn find_nodes_at_path<'a>(root: &'a [TreeNode], path: &[String]) -> Option<&'a [TreeNode]> {
    if path.is_empty() {
        return Some(root);
    }
    let mut current: &[TreeNode] = root;
    for seg in path {
        let mut next: Option<&[TreeNode]> = None;
        for node in current {
            if let TreeNode::Dir { name, children, .. } = node {
                if name == seg {
                    next = Some(children.as_slice());
                    break;
                }
            }
        }
        current = next?;
    }
    Some(current)
}

fn build_full_path_for(parent: &[String], name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{}/{}", parent.join("/"), name)
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
