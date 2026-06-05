use std::collections::HashSet;
use std::path::PathBuf;
use egui::*;
use ue_mod_core::{PakEntry, TreeNode, FontReplacement};

use crate::dev_mode::FilterCache;

#[derive(Default)]
pub struct TreeViewState {
    pub selected_path: Option<String>,
    pub multi_selected: HashSet<String>,
    pub expanded: HashSet<String>,
    pub filter: String,
    pub use_regex: bool,

    drag_start: Option<Pos2>,
    drag_current: Option<Pos2>,
}

impl TreeViewState {
    pub fn is_selected(&self, path: &str) -> bool {
        self.selected_path.as_deref() == Some(path) || self.multi_selected.contains(path)
    }

    pub fn select(&mut self, path: String, multi: bool) {
        if multi {
            // toggle
            if !self.multi_selected.remove(&path) {
                self.multi_selected.insert(path);
            }
        } else {
            self.multi_selected.clear();
            self.selected_path = Some(path.clone());
            self.multi_selected.insert(path);
        }
    }
}

pub fn show_tree(
    ui: &mut Ui,
    nodes: &[TreeNode],
    state: &mut TreeViewState,
    open_locres: &mut Option<(String, PathBuf)>,
    add_font: &mut Option<FontReplacement>,
    batch_fonts: &mut Option<(PathBuf, Vec<String>)>,
    cache: &FilterCache,
    navigate_to: &mut Option<Vec<String>>,
) {
    ui.horizontal(|ui| {
        ui.label("搜尋:");
        ui.text_edit_singleline(&mut state.filter);
        if ui.small_button("X").clicked() {
            state.filter.clear();
        }
        ui.checkbox(&mut state.use_regex, "Regex");
    });

    ui.separator();

    if state.use_regex && !state.filter.is_empty() {
        if regex::RegexBuilder::new(&state.filter).case_insensitive(true).build().is_err() {
            ui.colored_label(Color32::RED, "Regex 語法錯誤，已忽略");
        }
    }

    let interact_rect = ui.available_rect_before_wrap();
    let response = ui.interact(interact_rect, ui.id().with("tree_drag"), Sense::drag());

    // 只在「左鍵」拖曳時才啟動框選，右鍵（context menu）不應清除已選取的範圍
    let is_right_click = ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Secondary));

    if response.drag_started() && response.interact_pointer_pos().is_some() && !is_right_click {
        state.drag_start = response.interact_pointer_pos();
        if !ui.input(|i| i.modifiers.ctrl || i.modifiers.shift) {
            state.multi_selected.clear();
            state.selected_path = None;
        }
    }

    if response.dragged() {
        state.drag_current = response.interact_pointer_pos();
    }

    if response.drag_stopped() {
        state.drag_start = None;
        state.drag_current = None;
    }

    // 繪製拖曳框
    let drag_rect = if let (Some(start), Some(curr)) = (state.drag_start, state.drag_current) {
        let rect = Rect::from_two_pos(start, curr);
        ui.painter().rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(100, 150, 255, 30));
        ui.painter().rect_stroke(rect, 0.0, Stroke::new(1.0, Color32::from_rgb(150, 200, 255)));
        Some(rect)
    } else {
        None
    };

    show_nodes(
        ui, nodes, state, "", cache,
        open_locres, add_font, batch_fonts, 0, drag_rect, navigate_to,
    );
}

fn show_nodes(
    ui: &mut Ui,
    nodes: &[TreeNode],
    state: &mut TreeViewState,
    parent_path: &str,
    cache: &FilterCache,
    open_locres: &mut Option<(String, PathBuf)>,
    add_font: &mut Option<FontReplacement>,
    batch_fonts: &mut Option<(PathBuf, Vec<String>)>,
    depth: usize,
    drag_rect: Option<Rect>,
    navigate_to: &mut Option<Vec<String>>,
) {
    let cache_active = cache.is_active();

    for node in nodes {
        match node {
            TreeNode::Dir { name, children, .. } => {
                let full_path = if parent_path.is_empty() {
                    name.clone()
                } else {
                    format!("{}/{}", parent_path, name)
                };

                if cache_active && !cache.matched_dirs.contains(&full_path) {
                    continue;
                }

                let is_expanded = state.expanded.contains(&full_path);
                let indent = depth as f32 * 12.0;

                let response = ui.horizontal(|ui| {
                    ui.add_space(indent);
                    let icon = if is_expanded { "[-]" } else { "[+]" };
                    let header = RichText::new(format!("{} {}", icon, name))
                        .color(Color32::from_rgb(180, 200, 255));
                    ui.selectable_label(false, header)
                });

                if response.inner.clicked() {
                    if is_expanded {
                        state.expanded.remove(&full_path);
                    } else {
                        state.expanded.insert(full_path.clone());
                    }
                    // 同步穿梭模式：點到資料夾就把右側「當下目錄」切到這層。
                    *navigate_to = Some(
                        full_path.split('/').filter(|s| !s.is_empty()).map(str::to_string).collect()
                    );
                }

                if is_expanded {
                    show_nodes(
                        ui, children, state, &full_path, cache,
                        open_locres, add_font, batch_fonts, depth + 1, drag_rect, navigate_to,
                    );
                }
            }

            TreeNode::File(entry) => {
                if cache_active && !cache.matched_files.contains(&entry.path) {
                    continue;
                }

                let indent = depth as f32 * 12.0;
                let icon = if entry.is_locres() { "[Text]" }
                    else if entry.is_font() { "[Font]" }
                    else { "[Other]" };

                let file_name = entry.file_name();
                let label = RichText::new(format!("{} {}", icon, file_name));
                let label = if entry.is_locres() {
                    label.color(Color32::from_rgb(150, 230, 150))
                } else if entry.is_font() {
                    label.color(Color32::from_rgb(230, 180, 100))
                } else {
                    label
                };

                let response = ui.horizontal(|ui| {
                    ui.add_space(indent);
                    let r = ui.selectable_label(state.is_selected(&entry.path), label);
                    if let Some(rect) = drag_rect {
                        if rect.intersects(r.rect) {
                            state.multi_selected.insert(entry.path.clone());
                        }
                    }
                    r
                });

                let response = response.inner;

                if response.clicked() {
                    let multi = ui.input(|i| i.modifiers.ctrl || i.modifiers.shift);
                    state.select(entry.path.clone(), multi);
                    // 同步穿梭模式：點到檔案時，把當下目錄切到該檔所在資料夾。
                    if !multi {
                        *navigate_to = Some(
                            parent_path.split('/').filter(|s| !s.is_empty())
                                .map(str::to_string).collect()
                        );
                    }
                }

                if response.double_clicked() && entry.is_locres() {
                    *open_locres = Some((entry.path.clone(), entry.pak.clone()));
                }

                response.context_menu(|ui| {
                    show_context_menu(ui, entry, state, open_locres, add_font, batch_fonts);
                });

                response.on_hover_text(&entry.path);
            }
        }
    }
}

fn show_context_menu(
    ui: &mut Ui,
    entry: &PakEntry,
    state: &TreeViewState,
    open_locres: &mut Option<(String, PathBuf)>,
    add_font: &mut Option<FontReplacement>,
    batch_fonts: &mut Option<(PathBuf, Vec<String>)>,
) {
    ui.label(RichText::new(entry.file_name()).strong());
    ui.label(RichText::new(&entry.path).small().color(Color32::GRAY));
    ui.separator();

    let multi_fonts: Vec<String> = state.multi_selected.iter()
        .filter(|p| p.ends_with(".ufont") || p.ends_with(".ttf") || p.ends_with(".otf"))
        .cloned()
        .collect();

    if multi_fonts.len() > 1 && entry.is_font() {
        if ui.button(format!("批量替換 {} 個選取的字體...", multi_fonts.len())).clicked() {
            if let Some(new_font) = rfd::FileDialog::new()
                .add_filter("字體檔案", &["ttf", "otf", "ufont"])
                .pick_file()
            {
                // 只回傳「替換字體 + 內部路徑清單」，source_pak 由 app.rs 走 tree 解析
                // （同一內部路徑可能對應多個 pak，必須由有 tree 上下文的呼叫端處理）。
                *batch_fonts = Some((new_font, multi_fonts));
            }
            ui.close_menu();
        }
    } else {
        if entry.is_locres() {
            if ui.button("開啟在地化編輯器").clicked() {
                *open_locres = Some((entry.path.clone(), entry.pak.clone()));
                ui.close_menu();
            }
            if ui.button("匯出 CSV...").clicked() {
                if let Some(_output) = rfd::FileDialog::new()
                    .add_filter("CSV", &["csv"])
                    .set_file_name(format!(
                        "{}.csv",
                        entry.file_name().strip_suffix(".locres").unwrap_or(entry.file_name())
                    ))
                    .save_file()
                {
                    *open_locres = Some((entry.path.clone(), entry.pak.clone()));
                }
                ui.close_menu();
            }
        }

        if entry.is_font() {
            if ui.button("替換字體...").clicked() {
                if let Some(new_font) = rfd::FileDialog::new()
                    .add_filter("字體檔案", &["ttf", "otf", "ufont"])
                    .pick_file()
                {
                    *add_font = Some(FontReplacement {
                        source_pak: entry.pak.clone(),
                        pak_path: entry.path.clone(),
                        replacement: new_font,
                    });
                }
                ui.close_menu();
            }
        }
    }

    ui.separator();

    if ui.button("複製路徑").clicked() {
        ui.output_mut(|o| o.copied_text = entry.path.clone());
        ui.close_menu();
    }

    if ui.button("複製 PAK 路徑").clicked() {
        ui.output_mut(|o| o.copied_text = entry.pak.display().to_string());
        ui.close_menu();
    }
}