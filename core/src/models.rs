use std::path::PathBuf;
use serde::{Deserialize, Serialize};

// ── PakEntry ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct PakEntry {
    pub pak: PathBuf,
    pub path: String,
    pub size: Option<u64>,
}

impl PakEntry {
    pub fn file_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    pub fn is_locres(&self) -> bool {
        self.path.ends_with(".locres")
    }

    pub fn is_font(&self) -> bool {
        self.path.ends_with(".ufont")
            || self.path.ends_with(".otf")
            || self.path.ends_with(".ttf")
    }
}

// ── TreeNode ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum TreeNode {
    Dir {
        name: String,
        children: Vec<TreeNode>,
        expanded: bool,
    },
    File(PakEntry),
}

impl TreeNode {
    pub fn name(&self) -> &str {
        match self {
            TreeNode::Dir { name, .. } => name,
            TreeNode::File(e) => e.file_name(),
        }
    }

    pub fn is_dir(&self) -> bool {
        matches!(self, TreeNode::Dir { .. })
    }
}

// ── BuildMode ─────────────────────────────────────────────────────────────────
// 優化點：加上 Copy — 這是沒有關聯資料的簡單枚舉，複製成本為零，
//         可消除整個專案中大量的 `.clone()` 呼叫。

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BuildMode {
    Pak,
    IoStore,
}

// ── BuildTarget ───────────────────────────────────────────────────────────────
// 同上，加上 Copy。

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BuildTarget {
    LocresOnly,
    FontsOnly,
    All,
}

// ── UEVersion /(Form Retoc and Repak)─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum UEVersion {
    UE4_0, UE4_1, UE4_2, UE4_3, UE4_4, UE4_5, UE4_6, UE4_7, UE4_8, UE4_9,
    UE4_10, UE4_11, UE4_12, UE4_13, UE4_14, UE4_15, UE4_16, UE4_17, UE4_18, UE4_19,
    UE4_20, UE4_21, UE4_22, UE4_23, UE4_24, UE4_25, UE4_26, UE4_27,
    UE5_0, UE5_1, UE5_2, UE5_3, UE5_4, UE5_5, UE5_6, UE5_7, UE5_8, UE5_9,
}

impl UEVersion {
    pub fn from_major_minor(major: i32, minor: i32) -> Option<Self> {
        let all = Self::all();
        match major {
            4 if minor <= 27 => Some(all[minor as usize]),
            5 if minor <= 7  => Some(all[28 + minor as usize]),
            _ => None,
        }
    }

    pub fn as_str(&self) -> String {
        if *self < Self::UE5_0 {
            format!("4.{}", *self as u32)
        } else {
            format!("5.{}", *self as u32 - Self::UE5_0 as u32)
        }
    }

    pub fn all() -> &'static [UEVersion] {
        &[
            UEVersion::UE4_0,  UEVersion::UE4_1,  UEVersion::UE4_2,  UEVersion::UE4_3,  UEVersion::UE4_4,
            UEVersion::UE4_5,  UEVersion::UE4_6,  UEVersion::UE4_7,  UEVersion::UE4_8,  UEVersion::UE4_9,
            UEVersion::UE4_10, UEVersion::UE4_11, UEVersion::UE4_12, UEVersion::UE4_13, UEVersion::UE4_14,
            UEVersion::UE4_15, UEVersion::UE4_16, UEVersion::UE4_17, UEVersion::UE4_18, UEVersion::UE4_19,
            UEVersion::UE4_20, UEVersion::UE4_21, UEVersion::UE4_22, UEVersion::UE4_23, UEVersion::UE4_24,
            UEVersion::UE4_25, UEVersion::UE4_26, UEVersion::UE4_27,
            UEVersion::UE5_0,  UEVersion::UE5_1,  UEVersion::UE5_2, UEVersion::UE5_3, UEVersion::UE5_4,
            UEVersion::UE5_5,  UEVersion::UE5_6,  UEVersion::UE5_7, UEVersion::UE5_8,
        ]
    }

    pub fn repak_version(&self) -> &'static str {
        if *self <= Self::UE4_2  { return "V2"; }
        if *self <= Self::UE4_15 { return "V3"; }
        if *self <= Self::UE4_19 { return "V4"; }
        if *self == Self::UE4_20 { return "V5"; }
        if *self == Self::UE4_21 { return "V7"; }
        if *self == Self::UE4_22 { return "V8A"; }
        if *self <= Self::UE4_24 { return "V8B"; }
        if *self == Self::UE4_25 { return "V9"; }
        "V11"
    }
    // ── UE4.27+ Default Use Oodle──────
    pub fn repak_compression(&self) -> &'static str {
        if *self >= Self::UE4_27 { "Oodle" } else { "Zlib" }
    }

    pub fn retoc_version_str(&self) -> String {
        if *self < Self::UE4_25 {
            "UE4_25".to_string()
        } else if *self < Self::UE5_0 {
            format!("UE4_{}", *self as u32)
        } else {
            format!("UE5_{}", *self as u32 - Self::UE5_0 as u32)
        }
    }
}

impl std::fmt::Display for UEVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "UE {}", self.as_str())
    }
}

// ── LocresEntry ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocresEntry {
    pub namespace: String,
    pub key: String,
    pub value: String,
    pub modified: Option<String>,
}

impl LocresEntry {
    pub fn effective_value(&self) -> &str {
        self.modified.as_deref().unwrap_or(&self.value)
    }

    pub fn is_modified(&self) -> bool {
        self.modified.is_some()
    }
}

// ── FontReplacement ───────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct FontReplacement {
    pub pak_path: String,
    pub replacement: PathBuf,
}

// ── StagingArea ───────────────────────────────────────────────────────────────

#[derive(Default, Clone, Debug)]
pub struct StagingArea {
    pub locres_edits: std::collections::HashMap<String, Vec<LocresEntry>>,
    pub font_replacements: Vec<FontReplacement>,
    pub extra_files: Vec<(String, PathBuf)>,
}

impl StagingArea {
    pub fn is_empty(&self) -> bool {
        self.locres_edits.is_empty()
            && self.font_replacements.is_empty()
            && self.extra_files.is_empty()
    }

    pub fn total_changes(&self) -> usize {
        // 以「檔案」為單位計數：一個 locres 檔案不論修改多少條目，僅算作 1 個
        self.locres_edits
            .values()
            .filter(|v| v.iter().any(|e| e.is_modified()))
            .count()
            + self.font_replacements.len()
            + self.extra_files.len()
    }
}

// ── BuildConfig ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct BuildConfig {
    pub mode: BuildMode,
    pub version: UEVersion,
    pub output_dir: PathBuf,
    pub mod_name: String,
    pub target: BuildTarget,
}

// ── LogEntry / LogLevel ───────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct LogEntry {
    pub level: LogLevel,
    pub message: String,
    pub timestamp: std::time::SystemTime,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LogLevel {
    Info,
    Success,
    Warning,
    Error,
}

impl LogLevel {
    pub fn label(&self) -> &'static str {
        match self {
            LogLevel::Info    => "INFO",
            LogLevel::Success => "OK",
            LogLevel::Warning => "WARN",
            LogLevel::Error   => "ERR",
        }
    }
}