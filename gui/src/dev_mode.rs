// ── 開發者模式：高效率篩選與外部 JSON 比對 ──────────────────────────────────
//
// 全檔案模式下節點數可達數十萬。原本的「每幀遞迴 + 雙向 substring」會把 UI 拖死。
// 這個模組提供兩個關鍵加速結構：
//  - `ExternalFilter`：用 basename 索引 + 全路徑 HashSet，把每筆 JSON path 比對降到 O(1)。
//  - `FilterCache`：把「樹中哪些 file/dir 通過篩選」算一次存起來，重繪時只做 HashSet lookup。
//
// 兩者都以「輸入未變則跳過重建」的方式控制重算成本。

use std::collections::{HashMap, HashSet};
use ue_mod_core::TreeNode;

// ── ExternalFilter ─────────────────────────────────────────────────────────

#[derive(Default)]
pub struct ExternalFilter {
    /// 原始 path 集合（保留給統計顯示用，不參與比對熱路徑）
    pub raw: HashSet<String>,
    /// 規範化後的完整 path（小寫，'/' 分隔）— 精確比對用
    full_paths: HashSet<String>,
    /// basename(小寫) → 該 basename 的所有完整 path（小寫）
    by_basename: HashMap<String, Vec<String>>,
}

impl ExternalFilter {
    pub fn from_paths(paths: HashSet<String>) -> Self {
        let mut full_paths: HashSet<String> = HashSet::with_capacity(paths.len());
        let mut by_basename: HashMap<String, Vec<String>> = HashMap::with_capacity(paths.len());

        for p in &paths {
            let lower = p.replace('\\', "/").to_lowercase();
            let basename = lower.rsplit('/').next().unwrap_or(&lower).to_string();
            by_basename.entry(basename).or_default().push(lower.clone());
            full_paths.insert(lower);
        }

        Self { raw: paths, full_paths, by_basename }
    }

    pub fn len(&self) -> usize {
        self.raw.len()
    }

    /// 路徑是否符合 JSON 篩選。寬容處理：
    ///   1. 完整路徑精確匹配（小寫）
    ///   2. basename 命中 → 對應候選做 ends_with 雙向比對
    pub fn matches(&self, pak_internal_path: &str) -> bool {
        let lower = pak_internal_path.replace('\\', "/").to_lowercase();
        if self.full_paths.contains(&lower) {
            return true;
        }
        let basename = lower.rsplit('/').next().unwrap_or(&lower);
        if let Some(candidates) = self.by_basename.get(basename) {
            for c in candidates {
                if lower.ends_with(c.as_str()) || c.ends_with(lower.as_str()) {
                    return true;
                }
            }
        }
        false
    }
}

// ── FilterCache ────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct FilterCache {
    /// 上次重建的指紋；輸入未變就跳過重建。
    last_filter: String,
    last_use_regex: bool,
    last_external_len: usize,
    last_tree_files: usize,

    /// 通過篩選的 file 完整路徑（即 PakEntry.path）
    pub matched_files: HashSet<String>,
    /// 通過篩選的 dir 完整路徑（不含 pak 根 → 子層的拼接路徑）
    pub matched_dirs: HashSet<String>,
    /// 通過篩選的檔案數量（顯示用）
    pub total_matches: usize,
}

impl FilterCache {
    /// 若指紋一致則跳過。否則重建 cache。
    /// `tree_file_count` 用於檢測樹本身是否變動（重新掃描）。
    pub fn rebuild_if_needed(
        &mut self,
        nodes: &[TreeNode],
        filter: &str,
        use_regex: bool,
        external: Option<&ExternalFilter>,
        tree_file_count: usize,
    ) {
        let ext_len = external.map(|e| e.len()).unwrap_or(0);
        if filter == self.last_filter
            && use_regex == self.last_use_regex
            && ext_len == self.last_external_len
            && tree_file_count == self.last_tree_files
            && (!filter.is_empty() || external.is_some() || self.last_tree_files != 0)
            // 第一次（last_tree_files==0 && tree_file_count==0）也視為已建（空集合）
        {
            // 已建：不重建
            // 但若初始狀態（無篩選、無 external、tree 也空）就直接跳過
            return;
        }

        self.last_filter = filter.to_string();
        self.last_use_regex = use_regex;
        self.last_external_len = ext_len;
        self.last_tree_files = tree_file_count;
        self.matched_files.clear();
        self.matched_dirs.clear();
        self.total_matches = 0;

        let regex = if use_regex && !filter.is_empty() {
            regex::RegexBuilder::new(filter).case_insensitive(true).build().ok()
        } else {
            None
        };
        let lower_filter = filter.to_lowercase();
        let regex_active = regex.is_some();
        let substring_active = !regex_active && !filter.is_empty();
        let any_filter_active = regex_active || substring_active || external.is_some();

        if !any_filter_active {
            // 無任何篩選 → 不需要 cache（顯示全部）。matched_* 維持空，外部呼叫端要把
            // 「無篩選」視為直通。
            return;
        }

        // 遞迴收集所有檔案，匹配的塞進 matched_files，並把祖先目錄加進 matched_dirs。
        let mut path_stack: Vec<String> = Vec::new();
        walk(
            nodes,
            &mut path_stack,
            &lower_filter,
            regex.as_ref(),
            external,
            substring_active,
            regex_active,
            &mut self.matched_files,
            &mut self.matched_dirs,
        );
        self.total_matches = self.matched_files.len();
    }

    /// 是否處於「有篩選作用中」的狀態。外部呼叫端據此判斷要查 cache 還是直通顯示。
    pub fn is_active(&self) -> bool {
        !self.last_filter.is_empty() || self.last_external_len > 0
    }
}

#[allow(clippy::too_many_arguments)]
fn walk(
    nodes: &[TreeNode],
    path_stack: &mut Vec<String>,
    lower_filter: &str,
    regex: Option<&regex::Regex>,
    external: Option<&ExternalFilter>,
    substring_active: bool,
    regex_active: bool,
    matched_files: &mut HashSet<String>,
    matched_dirs: &mut HashSet<String>,
) {
    for node in nodes {
        match node {
            TreeNode::Dir { name, children, .. } => {
                path_stack.push(name.clone());
                walk(
                    children, path_stack,
                    lower_filter, regex, external,
                    substring_active, regex_active,
                    matched_files, matched_dirs,
                );
                path_stack.pop();
            }
            TreeNode::File(entry) => {
                let full_path = &entry.path;
                let file_name = entry.file_name();

                let search_ok = if regex_active {
                    let re = regex.unwrap();
                    re.is_match(full_path) || re.is_match(file_name)
                } else if substring_active {
                    full_path.to_lowercase().contains(lower_filter)
                        || file_name.to_lowercase().contains(lower_filter)
                } else {
                    true
                };

                let external_ok = match external {
                    Some(ext) => ext.matches(full_path),
                    None => true,
                };

                if search_ok && external_ok {
                    matched_files.insert(full_path.clone());
                    // 加入所有祖先目錄路徑（path_stack 的逐層拼接）
                    let mut acc = String::new();
                    for seg in path_stack.iter() {
                        if !acc.is_empty() {
                            acc.push('/');
                        }
                        acc.push_str(seg);
                        matched_dirs.insert(acc.clone());
                    }
                }
            }
        }
    }
}

// ── JSON path 提取 ─────────────────────────────────────────────────────────

/// 從 JSON 檔案中提取所有 "path" / "Path" 欄位的字串值。
///
/// 支援的形式（不分 key 大小寫、不論巢狀層級）：
///   { "Path": "a/b.uasset" }
///   { "Path": ["a/b.uasset", "c/d.uasset"] }
///   { "items": [ { "path": "..." }, ... ] }
///
/// 若 JSON 解析失敗（例如含有重複 key），退回 regex 直掃 `"path": "..."` 字串值。
pub fn load_json_path_filter(file: &std::path::Path) -> anyhow::Result<HashSet<String>> {
    let data = std::fs::read_to_string(file)?;
    let mut out = HashSet::new();

    match serde_json::from_str::<serde_json::Value>(&data) {
        Ok(value) => collect_paths(&value, &mut out),
        Err(_) => collect_paths_regex(&data, &mut out),
    }

    Ok(out)
}

fn collect_paths(value: &serde_json::Value, out: &mut HashSet<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                if k.eq_ignore_ascii_case("path") {
                    match v {
                        serde_json::Value::String(s) => { out.insert(s.clone()); }
                        serde_json::Value::Array(arr) => {
                            for it in arr {
                                if let Some(s) = it.as_str() {
                                    out.insert(s.to_string());
                                }
                            }
                        }
                        _ => {}
                    }
                }
                collect_paths(v, out);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                collect_paths(v, out);
            }
        }
        _ => {}
    }
}

fn collect_paths_regex(data: &str, out: &mut HashSet<String>) {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r#""(?i:path)"\s*:\s*"((?:[^"\\]|\\.)*)""#).unwrap()
    });
    for caps in re.captures_iter(data) {
        let raw = &caps[1];
        let quoted = format!("\"{}\"", raw);
        match serde_json::from_str::<String>(&quoted) {
            Ok(s) => { out.insert(s); }
            Err(_) => { out.insert(raw.to_string()); }
        }
    }
}

// ── 樹中收集 PakEntry（批量導出與自動選取用）────────────────────────────────

pub fn collect_all_files<'a>(nodes: &'a [TreeNode], out: &mut Vec<&'a ue_mod_core::PakEntry>) {
    for node in nodes {
        match node {
            TreeNode::Dir { children, .. } => collect_all_files(children, out),
            TreeNode::File(e) => out.push(e),
        }
    }
}

pub fn count_files(nodes: &[TreeNode]) -> usize {
    let mut n = 0usize;
    for node in nodes {
        match node {
            TreeNode::Dir { children, .. } => n += count_files(children),
            TreeNode::File(_) => n += 1,
        }
    }
    n
}
