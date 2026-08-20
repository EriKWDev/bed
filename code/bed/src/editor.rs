use core::alloc::Allocator;
use std::fmt::Write as _;
use std::io::Read as _;
use std::io::Write as _;

use crate::buffer::*;
use crate::code_index::{
    CodeIndex, CodeSymbolKind, code_index_definition_for, code_index_definition_of_kind,
    code_index_empty, code_index_identifier_at, code_index_invalidate_edits, code_index_set_path,
    code_index_step,
};
use crate::diagnostics::{
    DiagnosticSeverity, Diagnostics, diagnostics_pending, diagnostics_poll, diagnostics_restart,
    diagnostics_start,
};
use crate::fuzzy::{FuzzyMatch, fuzzy_byte_matches, fuzzy_rank};
use crate::git::{
    GitGutter, git_gutter_adjust_edits, git_gutter_empty, git_gutter_flags, git_gutter_invalidate,
    git_gutter_line_added, git_gutter_line_modified, git_gutter_line_removed,
    git_gutter_next_change, git_gutter_pending, git_gutter_poll, git_gutter_set_path,
    git_gutter_step,
};
use crate::project::{
    ProjectDiscoveryState, ProjectFiles, project_discover, project_discovery_step,
};
use crate::rust_methods::{
    RustBorrowKind, RustMethodCorpus, RustMethodIndex, rust_binding_type_at,
    rust_call_argument_type, rust_explicit_type, rust_free_function_complete, rust_method_complete,
    rust_method_definition, rust_method_detail, rust_method_index_empty, rust_method_index_finish,
    rust_method_index_pending, rust_method_index_poll, rust_method_index_restart,
    rust_method_index_start, rust_method_name, rust_method_path, rust_namespace_root,
    rust_symbol_detail, rust_symbol_name, rust_symbol_owner, rust_symbol_path,
};
use crate::syntax::{
    SYNTAX_KIND_COUNT, SyntaxHighlighting, SyntaxKind, SyntaxSpan, syntax_highlighting_empty,
    syntax_highlighting_invalidate_edits, syntax_highlighting_set_path, syntax_highlighting_spans,
    syntax_highlighting_step,
};
use crate::terminal::{self, Key, Terminal};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Select,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionState {
    pub cursor: usize,
    pub anchor: usize,
}

pub struct EditAtom {
    pub before_start: usize,
    pub after_start: usize,
    pub deleted: std::ops::Range<usize>,
    pub inserted: std::ops::Range<usize>,
}

pub struct Edit {
    pub atoms: Vec<EditAtom>,
    pub deleted_bytes: Vec<u8>,
    pub inserted_bytes: Vec<u8>,
    pub before: Vec<SelectionState>,
    pub after: Vec<SelectionState>,
}

#[derive(Clone)]
pub struct Replacement {
    pub start: usize,
    pub end: usize,
    pub inserted: std::ops::Range<usize>,
}

pub struct EditTransaction {
    pub edits: Vec<Edit>,
}

pub struct Document {
    pub buffer: GapBuffer,
    pub syntax: SyntaxHighlighting,
    pub code_index: CodeIndex,
    pub git_gutter: GitGutter,
    pub path: Option<std::path::PathBuf>,
    pub cursor: usize,
    pub anchor: usize,
    pub secondary_selections: Vec<SelectionState>,
    pub insertion_points: Vec<usize>,
    pub preferred_column: usize,
    pub top_line: usize,
    pub modified: bool,
    pub undo: Vec<EditTransaction>,
    pub redo: Vec<EditTransaction>,
    pub active_transaction: Option<EditTransaction>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerKind {
    Files,
    Commands,
    SearchProject,
    DocumentSymbols,
    WorkspaceSymbols,
    References,
    DocumentDiagnostics,
    WorkspaceDiagnostics,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingKey {
    None,
    Space,
    SpaceTheme,
    Goto,
    Match,
    MatchAround,
    MatchInside,
    MatchSurround,
    MatchReplaceFrom,
    MatchReplaceTo(char),
    InsertLineAbove,
    InsertLineBelow,
}

#[derive(Clone, Copy)]
pub struct KeyHint {
    pub key: &'static str,
    pub description: &'static str,
}

const SPACE_KEY_HINTS: &[KeyHint] = &[
    KeyHint {
        key: "/",
        description: "global search",
    },
    KeyHint {
        key: "f",
        description: "file picker",
    },
    KeyHint {
        key: "t",
        description: "theme",
    },
    KeyHint {
        key: "s",
        description: "document symbols",
    },
    KeyHint {
        key: "S",
        description: "workspace symbols",
    },
    KeyHint {
        key: "d",
        description: "document diagnostics",
    },
    KeyHint {
        key: "D",
        description: "workspace diagnostics",
    },
    KeyHint {
        key: "a",
        description: "code actions",
    },
    KeyHint {
        key: "Y",
        description: "yank to clipboard",
    },
    KeyHint {
        key: "P",
        description: "paste clipboard after",
    },
    KeyHint {
        key: "R",
        description: "replace with clipboard",
    },
];
const THEME_KEY_HINTS: &[KeyHint] = &[
    KeyHint {
        key: "d/k",
        description: "kanagawa",
    },
    KeyHint {
        key: "b",
        description: "bogster",
    },
    KeyHint {
        key: "l",
        description: "everforest light",
    },
    KeyHint {
        key: "n",
        description: "noctis",
    },
    KeyHint {
        key: "v",
        description: "kaolin valley dark",
    },
];
const GOTO_KEY_HINTS: &[KeyHint] = &[
    KeyHint {
        key: "g",
        description: "file start",
    },
    KeyHint {
        key: "e",
        description: "last line",
    },
    KeyHint {
        key: "h",
        description: "line start",
    },
    KeyHint {
        key: "l",
        description: "line end",
    },
    KeyHint {
        key: "s",
        description: "first non-whitespace",
    },
    KeyHint {
        key: "a",
        description: "last accessed buffer",
    },
    KeyHint {
        key: "n/p",
        description: "next/previous buffer",
    },
    KeyHint {
        key: "j/k",
        description: "line down/up",
    },
    KeyHint {
        key: "t/c/b",
        description: "window top/center/bottom",
    },
    KeyHint {
        key: "d",
        description: "definition",
    },
    KeyHint {
        key: "r",
        description: "references",
    },
];
const MATCH_KEY_HINTS: &[KeyHint] = &[
    KeyHint {
        key: "a",
        description: "select around",
    },
    KeyHint {
        key: "i",
        description: "select inside",
    },
    KeyHint {
        key: "s",
        description: "surround add",
    },
    KeyHint {
        key: "r",
        description: "surround replace",
    },
];
const MATCH_AROUND_KEY_HINTS: &[KeyHint] = &[KeyHint {
    key: "f/<char>",
    description: "select around delimiter",
}];
const MATCH_INSIDE_KEY_HINTS: &[KeyHint] = &[KeyHint {
    key: "f/<char>",
    description: "select inside delimiter",
}];
const MATCH_SURROUND_KEY_HINTS: &[KeyHint] = &[KeyHint {
    key: "<char>",
    description: "add surround",
}];
const MATCH_REPLACE_KEY_HINTS: &[KeyHint] = &[KeyHint {
    key: "<char>",
    description: "choose surround",
}];
const INSERT_LINE_KEY_HINTS: &[KeyHint] = &[
    KeyHint {
        key: "Space",
        description: "insert blank line",
    },
    KeyHint {
        key: "g",
        description: "Git change",
    },
    KeyHint {
        key: "f",
        description: "function",
    },
    KeyHint {
        key: "d",
        description: "diagnostic",
    },
    KeyHint {
        key: "a",
        description: "argument",
    },
];

pub fn pending_key_hints(pending: PendingKey) -> &'static [KeyHint] {
    match pending {
        PendingKey::None => &[],
        PendingKey::Space => SPACE_KEY_HINTS,
        PendingKey::SpaceTheme => THEME_KEY_HINTS,
        PendingKey::Goto => GOTO_KEY_HINTS,
        PendingKey::Match => MATCH_KEY_HINTS,
        PendingKey::MatchAround => MATCH_AROUND_KEY_HINTS,
        PendingKey::MatchInside => MATCH_INSIDE_KEY_HINTS,
        PendingKey::MatchSurround => MATCH_SURROUND_KEY_HINTS,
        PendingKey::MatchReplaceFrom | PendingKey::MatchReplaceTo(_) => MATCH_REPLACE_KEY_HINTS,
        PendingKey::InsertLineAbove | PendingKey::InsertLineBelow => INSERT_LINE_KEY_HINTS,
    }
}

bitfield::bitfield! {
    pub struct EditorFlags: 1 {
        const AUTO_INDENTATION = 0;
        const AUTO_INDENT_SCOPES = 1;
        const AUTO_PAIRS = 2;
    }
}

pub struct EditorConfig {
    pub flags: EditorFlags,
    pub indentation_spaces: usize,
    pub scroll_margin_lines: usize,
}

pub struct Completion {
    pub bytes: Vec<u8>,
    pub matches: Vec<CompletionEntry>,
    pub selected: usize,
    pub prefix_start: usize,
    pub preview: bool,
}

#[derive(Clone, Copy)]
pub struct CompletionEntry {
    pub name_start: u32,
    pub name_end: u32,
    pub detail_start: u32,
    pub detail_end: u32,
    pub insertion_start: u32,
    pub insertion_end: u32,
    pub selection_start: u32,
    pub selection_end: u32,
    pub replacement_start: usize,
    pub symbol: u32,
    pub flags: u8,
}

const COMPLETION_POSTFIX: u8 = 1 << 0;

struct CompletionCandidate<'a> {
    name: &'a str,
    detail: &'a str,
    insertion: &'a [u8],
    selection: std::ops::Range<u32>,
    replacement_start: usize,
    symbol: u32,
    flags: u8,
}

pub struct Register {
    pub bytes: Vec<u8>,
    pub values: Vec<std::ops::Range<u32>>,
}

pub struct ClipboardPaste {
    pub bytes: Vec<u8>,
    pub available: bool,
}

pub struct WorkspaceSnapshot {
    pub project: ProjectFiles,
    pub fingerprint: u64,
}

pub struct FormatResult {
    pub document: usize,
    pub original: Vec<u8>,
    pub formatted: Vec<u8>,
    pub formatter: &'static str,
    pub success: bool,
}

pub struct Picker {
    pub kind: PickerKind,
    pub query: String,
    pub matches: Vec<FuzzyMatch>,
    pub selected: usize,
    pub first_visible: usize,
    pub tab_cycling: bool,
    pub original_theme: usize,
    pub theme_preview: bool,
    pub search_query: String,
    pub search_candidates: Vec<usize>,
    pub search_candidate_position: usize,
    pub search_scan_position: usize,
    pub search_complete: bool,
    pub search_seen: Vec<u64>,
    pub search_ranked: Vec<FuzzyMatch>,
    pub symbol_corpus: SearchCorpus,
    pub preview_corpus: SearchCorpus,
    pub symbol_candidates: Vec<usize>,
    pub rust_symbol_candidates: Vec<usize>,
    pub reference_targets: Vec<ReferenceTarget>,
    pub diagnostic_candidates: Vec<usize>,
    pub preview: Option<PickerPreviewCache>,
    pub preview_load_key: Option<PickerPreviewKey>,
    pub preview_load_task: Option<idno_std::micropool::OwnedTask<PickerPreviewLoad>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PickerPreviewKey {
    pub kind: PickerKind,
    pub item: usize,
}

pub struct PickerPreviewCache {
    pub key: PickerPreviewKey,
    pub buffer: GapBuffer,
    pub syntax: SyntaxHighlighting,
    pub target_line: usize,
    pub target_start: usize,
    pub target_end: usize,
    pub first_line_number: usize,
}

pub struct PickerPreviewLoad {
    pub key: PickerPreviewKey,
    pub path: std::path::PathBuf,
    pub bytes: Vec<u8>,
    pub target_line: usize,
    pub target_start: usize,
    pub target_end: usize,
    pub first_line_number: usize,
    pub available: bool,
}

#[derive(Clone, Copy)]
pub struct ReferenceTarget {
    pub project_file: u32,
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, Copy)]
pub struct ReferenceDefinition {
    pub identifier: Option<u32>,
    pub position: Option<usize>,
    pub rust_symbol: Option<usize>,
    pub workspace_symbol: Option<usize>,
    pub workspace: bool,
}

#[derive(Clone, Copy)]
pub struct SearchLine {
    pub project_file: u32,
    pub file_offset: u32,
    pub line_number: u32,
    pub text_start: u32,
    pub display_start: u32,
    pub display_end: u32,
}

pub struct SearchCorpus {
    pub bytes: Vec<u8>,
    pub lines: Vec<SearchLine>,
    pub identifiers: Vec<SearchIdentifier>,
    pub symbols: Vec<SearchSymbol>,
}

#[derive(Clone, Copy)]
pub struct SearchSymbol {
    pub name_start: u32,
    pub name_end: u32,
    pub project_file: u32,
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, Copy)]
pub struct SearchIdentifier {
    pub name_start: u32,
    pub name_end: u32,
    pub project_file: u32,
    pub file_start: u32,
    pub file_end: u32,
    pub line: u32,
}

#[derive(Clone, Copy)]
struct PickerPreview<'a> {
    corpus: &'a SearchCorpus,
    target: usize,
    file_start: usize,
    file_end: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchKind {
    Document,
    Selection,
}

pub struct SearchSession {
    pub kind: SearchKind,
    pub document: usize,
    pub query: String,
    pub original_selections: Vec<SelectionState>,
    pub matches: Vec<SelectionState>,
    pub selected: usize,
    pub corpus: SearchCorpus,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Jump {
    pub document: usize,
    pub cursor: usize,
    pub anchor: usize,
    pub top_line: usize,
}

pub struct Editor {
    pub documents: Vec<Document>,
    pub current: usize,
    pub project: ProjectFiles,
    pub mode: Mode,
    pub picker: Option<Picker>,
    pub pending_key: PendingKey,
    pub pending_count: usize,
    pub last_accessed_document: Option<usize>,
    pub config: EditorConfig,
    pub viewport_height: usize,
    pub terminal_width: usize,
    pub terminal_height: usize,
    pub jumps: Vec<Jump>,
    pub jump_position: usize,
    pub quit_warning: bool,
    pub quit_requested: bool,
    pub status: String,
    pub frame: Vec<u8>,
    pub present_frame: Vec<u8>,
    pub rendered_row_hashes: Vec<u64>,
    pub rendered_theme: usize,
    pub rendered_picker: bool,
    pub rendered_overlay_start: Option<usize>,
    pub theme: usize,
    pub search_query: String,
    pub search_matches: Vec<SelectionState>,
    pub search_position: usize,
    pub search: Option<SearchSession>,
    pub project_search: Option<SearchCorpus>,
    pub project_search_task: Option<idno_std::micropool::OwnedTask<SearchCorpus>>,
    pub project_discovery: Option<ProjectDiscoveryState>,
    pub workspace_refresh_task: Option<idno_std::micropool::OwnedTask<WorkspaceSnapshot>>,
    pub workspace_fingerprint: u64,
    pub workspace_refresh_at: std::time::Instant,
    pub rust_methods: RustMethodIndex,
    pub completion: Option<Completion>,
    pub register: Register,
    pub clipboard_copy_task: Option<idno_std::micropool::OwnedTask<bool>>,
    pub clipboard_paste_task: Option<idno_std::micropool::OwnedTask<ClipboardPaste>>,
    pub clipboard_paste_replaces: bool,
    pub diagnostics: Diagnostics,
    pub format_task: Option<idno_std::micropool::OwnedTask<FormatResult>>,
}

const COMMANDS: &[&str] = &[
    "write",
    "w",
    "write!",
    "w!",
    "quit",
    "q",
    "quit!",
    "q!",
    "write-quit",
    "wq",
    "write-quit!",
    "wq!",
    "write-all",
    "wa",
    "write-all!",
    "wa!",
    "write-quit-all",
    "wqa",
    "xa",
    "write-quit-all!",
    "wqa!",
    "xa!",
    "quit-all",
    "qa",
    "quit-all!",
    "qa!",
    "open",
    "o",
    "edit",
    "e",
    "new",
    "n",
    "buffer-close",
    "bc",
    "bclose",
    "buffer-close!",
    "bc!",
    "bclose!",
    "buffer-close-others",
    "bco",
    "buffer-close-others!",
    "bco!",
    "buffer-close-all",
    "bca",
    "buffer-close-all!",
    "bca!",
    "undo",
    "redo",
    "buffer-next",
    "bn",
    "buffer-previous",
    "bp",
    "theme",
    "t",
    "reload-files",
    "reload",
    "reload!",
    "rl",
    "rl!",
    "reload-all",
    "reload-all!",
    "rla",
    "rla!",
    "format",
    "toggle-auto-indentation",
    "toggle-auto-indent-scopes",
    "toggle-auto-pairs",
];

const RUST_COMPLETION_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while",
];

pub struct Theme {
    pub name: &'static str,
    pub normal: &'static [u8],
    pub gutter: &'static [u8],
    pub status: &'static [u8],
    pub selection_background: &'static [u8],
    pub preview_line_background: &'static [u8],
    pub cursor: &'static [u8],
    pub syntax: [&'static [u8]; SYNTAX_KIND_COUNT],
    pub git_added: &'static [u8],
    pub git_modified: &'static [u8],
    pub git_removed: &'static [u8],
    pub picker_selected: &'static [u8],
    pub cursor_color: &'static [u8],
}

const THEME_NAMES: &[&str] = &[
    "kanagawa",
    "bogster",
    "everforest_light",
    "noctis",
    "kaolin-valley-dark",
];

const THEMES: &[Theme] = &[
    Theme {
        name: "kanagawa",
        normal: b"\x1b[38;2;220;215;186m\x1b[48;2;31;31;40m",
        gutter: b"\x1b[38;2;114;113;105m\x1b[48;2;31;31;40m",
        status: b"\x1b[38;2;220;215;186m\x1b[48;2;42;42;55m",
        selection_background: b"\x1b[48;2;45;79;103m",
        preview_line_background: b"\x1b[48;2;38;39;50m",
        cursor: b"\x1b[38;2;31;31;40m\x1b[48;2;200;192;147m",
        syntax: [
            b"\x1b[38;2;114;113;105m\x1b[48;2;31;31;40m",
            b"\x1b[38;2;228;104;118m\x1b[48;2;31;31;40m",
            b"\x1b[38;2;152;187;108m\x1b[48;2;31;31;40m",
            b"\x1b[38;2;255;160;102m\x1b[48;2;31;31;40m",
            b"\x1b[38;2;122;168;159m\x1b[48;2;31;31;40m",
            b"\x1b[38;2;126;156;216m\x1b[48;2;31;31;40m",
            b"\x1b[38;2;230;195;132m\x1b[48;2;31;31;40m",
            b"\x1b[38;2;149;127;184m\x1b[48;2;31;31;40m",
            b"\x1b[38;2;255;160;102m\x1b[48;2;31;31;40m",
            b"\x1b[38;2;149;127;184m\x1b[48;2;31;31;40m",
            b"\x1b[38;2;228;104;118m\x1b[48;2;31;31;40m",
            b"\x1b[38;2;230;195;132m\x1b[48;2;31;31;40m",
            b"\x1b[38;2;200;192;147m\x1b[48;2;31;31;40m",
            b"\x1b[38;2;147;146;134m\x1b[48;2;31;31;40m",
            b"\x1b[38;2;230;195;132m\x1b[48;2;31;31;40m",
            b"\x1b[38;2;126;156;216m\x1b[48;2;31;31;40m",
            b"\x1b[38;2;255;160;102m\x1b[48;2;31;31;40m",
            b"\x1b[1;38;2;255;70;85m\x1b[48;2;31;31;40m",
        ],
        git_added: b"\x1b[38;2;152;187;108m\x1b[48;2;31;31;40m",
        git_modified: b"\x1b[38;2;255;160;102m\x1b[48;2;31;31;40m",
        git_removed: b"\x1b[38;2;228;104;118m\x1b[48;2;31;31;40m",
        picker_selected: b"\x1b[38;2;31;31;40m\x1b[48;2;126;156;216m",
        cursor_color: b"\x1b]12;#c8c093\x07",
    },
    Theme {
        name: "bogster",
        normal: b"\x1b[38;2;192;192;192m\x1b[48;2;18;18;18m",
        gutter: b"\x1b[38;2;100;100;100m\x1b[48;2;18;18;18m",
        status: b"\x1b[38;2;230;230;230m\x1b[48;2;45;45;45m",
        selection_background: b"\x1b[48;2;62;62;78m",
        preview_line_background: b"\x1b[48;2;31;31;36m",
        cursor: b"\x1b[38;2;18;18;18m\x1b[48;2;255;215;95m",
        syntax: [
            b"\x1b[38;2;105;105;105m\x1b[48;2;18;18;18m",
            b"\x1b[38;2;255;95;135m\x1b[48;2;18;18;18m",
            b"\x1b[38;2;135;215;135m\x1b[48;2;18;18;18m",
            b"\x1b[38;2;255;175;95m\x1b[48;2;18;18;18m",
            b"\x1b[38;2;95;215;215m\x1b[48;2;18;18;18m",
            b"\x1b[38;2;135;175;255m\x1b[48;2;18;18;18m",
            b"\x1b[38;2;255;215;95m\x1b[48;2;18;18;18m",
            b"\x1b[38;2;175;135;255m\x1b[48;2;18;18;18m",
            b"\x1b[38;2;255;175;95m\x1b[48;2;18;18;18m",
            b"\x1b[38;2;175;135;255m\x1b[48;2;18;18;18m",
            b"\x1b[38;2;255;95;135m\x1b[48;2;18;18;18m",
            b"\x1b[38;2;255;215;95m\x1b[48;2;18;18;18m",
            b"\x1b[38;2;215;175;95m\x1b[48;2;18;18;18m",
            b"\x1b[38;2;135;135;135m\x1b[48;2;18;18;18m",
            b"\x1b[38;2;255;215;95m\x1b[48;2;18;18;18m",
            b"\x1b[38;2;135;175;255m\x1b[48;2;18;18;18m",
            b"\x1b[38;2;255;175;95m\x1b[48;2;18;18;18m",
            b"\x1b[1;38;2;255;55;75m\x1b[48;2;18;18;18m",
        ],
        git_added: b"\x1b[38;2;135;215;135m\x1b[48;2;18;18;18m",
        git_modified: b"\x1b[38;2;255;175;95m\x1b[48;2;18;18;18m",
        git_removed: b"\x1b[38;2;255;95;135m\x1b[48;2;18;18;18m",
        picker_selected: b"\x1b[38;2;18;18;18m\x1b[48;2;102;204;153m",
        cursor_color: b"\x1b]12;#ffd75f\x07",
    },
    Theme {
        name: "everforest_light",
        normal: b"\x1b[38;2;92;106;114m\x1b[48;2;253;246;227m",
        gutter: b"\x1b[38;2;147;157;148m\x1b[48;2;253;246;227m",
        status: b"\x1b[38;2;79;91;88m\x1b[48;2;229;223;204m",
        selection_background: b"\x1b[48;2;211;222;203m",
        preview_line_background: b"\x1b[48;2;240;235;218m",
        cursor: b"\x1b[38;2;253;246;227m\x1b[48;2;130;150;116m",
        syntax: [
            b"\x1b[38;2;147;157;148m\x1b[48;2;253;246;227m",
            b"\x1b[38;2;229;112;110m\x1b[48;2;253;246;227m",
            b"\x1b[38;2;141;161;118m\x1b[48;2;253;246;227m",
            b"\x1b[38;2;229;165;100m\x1b[48;2;253;246;227m",
            b"\x1b[38;2;53;167;124m\x1b[48;2;253;246;227m",
            b"\x1b[38;2;62;121;144m\x1b[48;2;253;246;227m",
            b"\x1b[38;2;223;160;55m\x1b[48;2;253;246;227m",
            b"\x1b[38;2;160;107;155m\x1b[48;2;253;246;227m",
            b"\x1b[38;2;229;165;100m\x1b[48;2;253;246;227m",
            b"\x1b[38;2;160;107;155m\x1b[48;2;253;246;227m",
            b"\x1b[38;2;229;112;110m\x1b[48;2;253;246;227m",
            b"\x1b[38;2;223;160;55m\x1b[48;2;253;246;227m",
            b"\x1b[38;2;130;150;116m\x1b[48;2;253;246;227m",
            b"\x1b[38;2;147;157;148m\x1b[48;2;253;246;227m",
            b"\x1b[38;2;223;160;55m\x1b[48;2;253;246;227m",
            b"\x1b[38;2;62;121;144m\x1b[48;2;253;246;227m",
            b"\x1b[38;2;229;165;100m\x1b[48;2;253;246;227m",
            b"\x1b[1;38;2;205;55;55m\x1b[48;2;253;246;227m",
        ],
        git_added: b"\x1b[38;2;141;161;118m\x1b[48;2;253;246;227m",
        git_modified: b"\x1b[38;2;229;165;100m\x1b[48;2;253;246;227m",
        git_removed: b"\x1b[38;2;229;112;110m\x1b[48;2;253;246;227m",
        picker_selected: b"\x1b[38;2;253;246;227m\x1b[48;2;141;161;118m",
        cursor_color: b"\x1b]12;#829674\x07",
    },
    Theme {
        name: "noctis",
        normal: b"\x1b[38;2;205;213;223m\x1b[48;2;10;28;38m",
        gutter: b"\x1b[38;2;75;103;117m\x1b[48;2;10;28;38m",
        status: b"\x1b[38;2;205;213;223m\x1b[48;2;20;48;61m",
        selection_background: b"\x1b[48;2;33;73;92m",
        preview_line_background: b"\x1b[48;2;15;39;50m",
        cursor: b"\x1b[38;2;10;28;38m\x1b[48;2;73;214;183m",
        syntax: [
            b"\x1b[38;2;75;103;117m\x1b[48;2;10;28;38m",
            b"\x1b[38;2;239;123;143m\x1b[48;2;10;28;38m",
            b"\x1b[38;2;166;218;149m\x1b[48;2;10;28;38m",
            b"\x1b[38;2;247;194;119m\x1b[48;2;10;28;38m",
            b"\x1b[38;2;73;214;183m\x1b[48;2;10;28;38m",
            b"\x1b[38;2;130;170;255m\x1b[48;2;10;28;38m",
            b"\x1b[38;2;229;192;123m\x1b[48;2;10;28;38m",
            b"\x1b[38;2;192;153;255m\x1b[48;2;10;28;38m",
            b"\x1b[38;2;247;194;119m\x1b[48;2;10;28;38m",
            b"\x1b[38;2;192;153;255m\x1b[48;2;10;28;38m",
            b"\x1b[38;2;239;123;143m\x1b[48;2;10;28;38m",
            b"\x1b[38;2;229;192;123m\x1b[48;2;10;28;38m",
            b"\x1b[38;2;130;170;255m\x1b[48;2;10;28;38m",
            b"\x1b[38;2;75;103;117m\x1b[48;2;10;28;38m",
            b"\x1b[38;2;229;192;123m\x1b[48;2;10;28;38m",
            b"\x1b[38;2;130;170;255m\x1b[48;2;10;28;38m",
            b"\x1b[38;2;247;194;119m\x1b[48;2;10;28;38m",
            b"\x1b[1;38;2;255;70;90m\x1b[48;2;10;28;38m",
        ],
        git_added: b"\x1b[38;2;166;218;149m\x1b[48;2;10;28;38m",
        git_modified: b"\x1b[38;2;247;194;119m\x1b[48;2;10;28;38m",
        git_removed: b"\x1b[38;2;239;123;143m\x1b[48;2;10;28;38m",
        picker_selected: b"\x1b[38;2;10;28;38m\x1b[48;2;73;214;183m",
        cursor_color: b"\x1b]12;#49d6b7\x07",
    },
    Theme {
        name: "kaolin-valley-dark",
        normal: b"\x1b[38;2;224;223;219m\x1b[48;2;38;36;48m",
        gutter: b"\x1b[38;2;112;106;128m\x1b[48;2;38;36;48m",
        status: b"\x1b[38;2;224;223;219m\x1b[48;2;55;51;69m",
        selection_background: b"\x1b[48;2;73;62;91m",
        preview_line_background: b"\x1b[48;2;48;44;60m",
        cursor: b"\x1b[38;2;38;36;48m\x1b[48;2;205;145;165m",
        syntax: [
            b"\x1b[38;2;112;106;128m\x1b[48;2;38;36;48m",
            b"\x1b[38;2;205;145;165m\x1b[48;2;38;36;48m",
            b"\x1b[38;2;126;177;139m\x1b[48;2;38;36;48m",
            b"\x1b[38;2;217;169;108m\x1b[48;2;38;36;48m",
            b"\x1b[38;2;109;181;180m\x1b[48;2;38;36;48m",
            b"\x1b[38;2;126;161;210m\x1b[48;2;38;36;48m",
            b"\x1b[38;2;224;193;137m\x1b[48;2;38;36;48m",
            b"\x1b[38;2;164;138;212m\x1b[48;2;38;36;48m",
            b"\x1b[38;2;217;169;108m\x1b[48;2;38;36;48m",
            b"\x1b[38;2;164;138;212m\x1b[48;2;38;36;48m",
            b"\x1b[38;2;205;145;165m\x1b[48;2;38;36;48m",
            b"\x1b[38;2;224;193;137m\x1b[48;2;38;36;48m",
            b"\x1b[38;2;126;161;210m\x1b[48;2;38;36;48m",
            b"\x1b[38;2;112;106;128m\x1b[48;2;38;36;48m",
            b"\x1b[38;2;224;193;137m\x1b[48;2;38;36;48m",
            b"\x1b[38;2;126;161;210m\x1b[48;2;38;36;48m",
            b"\x1b[38;2;217;169;108m\x1b[48;2;38;36;48m",
            b"\x1b[1;38;2;255;65;85m\x1b[48;2;38;36;48m",
        ],
        git_added: b"\x1b[38;2;126;177;139m\x1b[48;2;38;36;48m",
        git_modified: b"\x1b[38;2;217;169;108m\x1b[48;2;38;36;48m",
        git_removed: b"\x1b[38;2;205;145;165m\x1b[48;2;38;36;48m",
        picker_selected: b"\x1b[38;2;38;36;48m\x1b[48;2;205;145;165m",
        cursor_color: b"\x1b]12;#cd91a5\x07",
    },
];

pub fn editor_open(path: Option<std::path::PathBuf>) -> std::io::Result<Editor> {
    let (root, initial_file, open_picker) = match path {
        Some(path) if path.is_dir() => (path, None, true),
        Some(path) => {
            let path = std::fs::canonicalize(&path).unwrap_or_else(|_| {
                if path.is_absolute() {
                    path
                } else {
                    std::env::current_dir()
                        .unwrap_or_else(|_| std::path::PathBuf::from("."))
                        .join(path)
                }
            });
            let root = path
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .to_path_buf();
            (root, Some(path), false)
        }
        None => (
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            None,
            false,
        ),
    };
    let discovery = project_discover(root, 1024);
    let project = discovery.project;
    let rust_methods = if cfg!(test) {
        rust_method_index_empty()
    } else {
        rust_method_index_start(&project.root, std::sync::Arc::clone(&project.paths))
    };
    let diagnostics = diagnostics_start(&project.root);
    let ignore_status = project
        .ignore_file
        .as_ref()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .map(|name| format!("using {name}"));
    let document = match initial_file {
        Some(path) => match document_open(Some(path)) {
            Ok(document) => document,
            Err(error) => return Err(error),
        },
        None => document_empty(),
    };
    let project_discovery = discovery.state;
    let (project_search, project_search_task) = if !discovery.complete {
        (None, Some(project_search_spawn(&project)))
    } else if project_search_should_build_inline(&project) {
        (
            Some(project_search_index(&project.paths, &project.labels)),
            None,
        )
    } else {
        (None, Some(project_search_spawn(&project)))
    };
    let mut editor = Editor {
        documents: vec![document],
        current: 0,
        project,
        mode: Mode::Normal,
        picker: None,
        pending_key: PendingKey::None,
        pending_count: 0,
        last_accessed_document: None,
        config: EditorConfig {
            flags: EditorFlags::AUTO_INDENTATION
                | EditorFlags::AUTO_INDENT_SCOPES
                | EditorFlags::AUTO_PAIRS,
            indentation_spaces: 4,
            scroll_margin_lines: 3,
        },
        viewport_height: 23,
        terminal_width: 0,
        terminal_height: 0,
        jumps: Vec::new(),
        jump_position: 0,
        quit_warning: false,
        quit_requested: false,
        status: ignore_status.unwrap_or_default(),
        frame: Vec::with_capacity(32 * 1024),
        present_frame: Vec::with_capacity(8 * 1024),
        rendered_row_hashes: Vec::new(),
        rendered_theme: usize::MAX,
        rendered_picker: false,
        rendered_overlay_start: None,
        theme: 0,
        search_query: String::new(),
        search_matches: Vec::new(),
        search_position: 0,
        search: None,
        project_search,
        project_search_task,
        project_discovery,
        workspace_refresh_task: None,
        workspace_fingerprint: 0,
        workspace_refresh_at: std::time::Instant::now() + std::time::Duration::from_secs(2),
        rust_methods,
        completion: None,
        register: Register {
            bytes: Vec::with_capacity(1024),
            values: Vec::with_capacity(8),
        },
        clipboard_copy_task: None,
        clipboard_paste_task: None,
        clipboard_paste_replaces: false,
        diagnostics,
        format_task: None,
    };
    if open_picker {
        editor_open_picker(&mut editor, PickerKind::Files);
    }
    Ok(editor)
}

pub fn document_empty() -> Document {
    Document {
        buffer: buffer_from_bytes(&[]),
        syntax: syntax_highlighting_empty(),
        code_index: code_index_empty(),
        git_gutter: git_gutter_empty(),
        path: None,
        cursor: 0,
        anchor: 0,
        secondary_selections: Vec::new(),
        insertion_points: Vec::new(),
        preferred_column: 0,
        top_line: 0,
        modified: false,
        undo: Vec::new(),
        redo: Vec::new(),
        active_transaction: None,
    }
}

pub fn document_open(path: Option<std::path::PathBuf>) -> std::io::Result<Document> {
    let source = match &path {
        Some(path) if path.exists() => match std::fs::read(path) {
            Ok(source) => source,
            Err(error) => return Err(error),
        },
        _ => Vec::new(),
    };
    let mut document = document_empty();
    document.buffer = buffer_from_bytes(&source);
    document.path = path;
    syntax_highlighting_set_path(&mut document.syntax, document.path.as_deref());
    code_index_set_path(&mut document.code_index, document.path.as_deref());
    git_gutter_set_path(&mut document.git_gutter, document.path.as_deref());
    syntax_highlighting_step(
        &document.buffer,
        &mut document.syntax,
        256 * 1024,
        std::time::Duration::from_millis(1),
    );
    code_index_step(
        &document.buffer,
        &mut document.code_index,
        256 * 1024,
        std::time::Duration::from_millis(1),
    );
    Ok(document)
}

#[inline]
pub fn editor_document(editor: &Editor) -> &Document {
    &editor.documents[editor.current]
}

#[inline]
pub fn editor_document_mut(editor: &mut Editor) -> &mut Document {
    &mut editor.documents[editor.current]
}

pub fn editor_run(editor: &mut Editor, terminal: &mut Terminal) -> std::io::Result<()> {
    let mut input_changed = true;
    loop {
        let background_changed = editor_poll_project_discovery(editor)
            | editor_poll_workspace_refresh(editor)
            | editor_poll_project_search(editor)
            | editor_step_project_search(editor)
            | editor_step_syntax_highlighting(editor)
            | editor_poll_picker_preview_load(editor)
            | editor_step_picker_preview(editor)
            | editor_step_code_index(editor)
            | editor_step_git_gutter(editor)
            | editor_poll_rust_methods(editor)
            | editor_poll_clipboard(editor)
            | editor_poll_format(editor)
            | diagnostics_poll(&mut editor.diagnostics);
        if input_changed || background_changed {
            match editor_render(editor, terminal) {
                Ok(()) => {}
                Err(error) => return Err(error),
            }
        }
        let timeout = if editor_incremental_work_pending(editor) {
            0
        } else if editor_background_work_pending(editor) {
            16
        } else {
            100
        };
        let key = match terminal::terminal_read_key_timeout(terminal, timeout) {
            Ok(Some(key)) => key,
            Ok(None) => {
                let size = terminal::terminal_size(terminal);
                input_changed = size != (editor.terminal_width, editor.terminal_height);
                continue;
            }
            Err(error) => return Err(error),
        };
        input_changed = true;
        if editor_handle_key(editor, key) {
            return Ok(());
        }
    }
}

pub fn editor_handle_key(editor: &mut Editor, key: Key) -> bool {
    profiling::function_scope!();
    editor.status.clear();
    if editor.picker.is_some() {
        return editor_handle_picker_key(editor, key);
    }
    if editor.search.is_some() {
        return editor_handle_search_key(editor, key);
    }

    if editor.pending_key != PendingKey::None {
        let pending = editor.pending_key;
        if pending == PendingKey::Goto
            && let Key::Character(character) = key
            && let Some(digit) = character.to_digit(10)
        {
            editor.pending_count = editor
                .pending_count
                .saturating_mul(10)
                .saturating_add(digit as usize);
            return false;
        }
        let pending_count = editor.pending_count;
        editor.pending_count = 0;
        editor.pending_key = PendingKey::None;
        match (pending, key) {
            (PendingKey::Space, Key::Character('f')) => {
                editor_open_picker(editor, PickerKind::Files)
            }
            (PendingKey::Space, Key::Character('t')) => editor.pending_key = PendingKey::SpaceTheme,
            (PendingKey::Space, Key::Character('/')) => {
                editor_open_picker(editor, PickerKind::SearchProject)
            }
            (PendingKey::Space, Key::Character('s')) => {
                editor_open_picker(editor, PickerKind::DocumentSymbols)
            }
            (PendingKey::Space, Key::Character('S')) => {
                editor_open_picker(editor, PickerKind::WorkspaceSymbols)
            }
            (PendingKey::Space, Key::Character('d')) => {
                editor_open_picker(editor, PickerKind::DocumentDiagnostics)
            }
            (PendingKey::Space, Key::Character('D')) => {
                editor_open_picker(editor, PickerKind::WorkspaceDiagnostics)
            }
            (PendingKey::Space, Key::Character('a')) => editor_fill_struct_fields(editor),
            (PendingKey::Space, Key::Character('Y')) => editor_yank_system(editor),
            (PendingKey::Space, Key::Character('P')) => editor_paste_system(editor),
            (PendingKey::Space, Key::Character('R')) => editor_replace_system(editor),
            (PendingKey::SpaceTheme, Key::Character('d' | 'k')) => editor_set_theme(editor, 0),
            (PendingKey::SpaceTheme, Key::Character('b')) => editor_set_theme(editor, 1),
            (PendingKey::SpaceTheme, Key::Character('l')) => editor_set_theme(editor, 2),
            (PendingKey::SpaceTheme, Key::Character('n')) => editor_set_theme(editor, 3),
            (PendingKey::SpaceTheme, Key::Character('v')) => editor_set_theme(editor, 4),
            (PendingKey::Goto, Key::Character('g')) => {
                if pending_count == 0 {
                    editor_goto_file_start(editor);
                } else {
                    editor_goto_line(editor, pending_count.saturating_sub(1));
                }
            }
            (PendingKey::Goto, Key::Character('e')) => editor_goto_last_line(editor),
            (PendingKey::Goto, Key::Character('h')) => editor_move_line_boundary(editor, false),
            (PendingKey::Goto, Key::Character('l')) => editor_move_line_boundary(editor, true),
            (PendingKey::Goto, Key::Character('s')) => editor_goto_first_nonwhitespace(editor),
            (PendingKey::Goto, Key::Character('a')) => editor_goto_last_accessed_document(editor),
            (PendingKey::Goto, Key::Character('d')) => editor_goto_definition(editor),
            (PendingKey::Goto, Key::Character('r')) => editor_select_references(editor),
            (PendingKey::Goto, Key::Character('n')) => {
                editor_switch_document(editor, (editor.current + 1) % editor.documents.len())
            }
            (PendingKey::Goto, Key::Character('p')) => editor_switch_document(
                editor,
                editor
                    .current
                    .checked_sub(1)
                    .unwrap_or(editor.documents.len() - 1),
            ),
            (PendingKey::Goto, Key::Character('j')) => editor_move_vertical(editor, true),
            (PendingKey::Goto, Key::Character('k')) => editor_move_vertical(editor, false),
            (PendingKey::Goto, Key::Character('t')) => editor_goto_window_line(editor, 0),
            (PendingKey::Goto, Key::Character('c')) => editor_goto_window_line(editor, 1),
            (PendingKey::Goto, Key::Character('b')) => editor_goto_window_line(editor, 2),
            (PendingKey::Match, Key::Character('a')) => {
                editor.pending_key = PendingKey::MatchAround
            }
            (PendingKey::Match, Key::Character('i')) => {
                editor.pending_key = PendingKey::MatchInside
            }
            (PendingKey::Match, Key::Character('s')) => {
                editor.pending_key = PendingKey::MatchSurround
            }
            (PendingKey::Match, Key::Character('r')) => {
                editor.pending_key = PendingKey::MatchReplaceFrom
            }
            (PendingKey::MatchAround, Key::Character(delimiter)) => {
                if delimiter == 'f' {
                    editor_select_enclosing_function(editor, false);
                } else {
                    editor_select_surrounding(editor, delimiter, false);
                }
            }
            (PendingKey::MatchInside, Key::Character(delimiter)) => {
                if delimiter == 'f' {
                    editor_select_enclosing_function(editor, true);
                } else {
                    editor_select_surrounding(editor, delimiter, true);
                }
            }
            (PendingKey::MatchSurround, Key::Character(delimiter)) => {
                editor_add_surround(editor, delimiter)
            }
            (PendingKey::MatchReplaceFrom, Key::Character(delimiter)) => {
                editor.pending_key = PendingKey::MatchReplaceTo(delimiter)
            }
            (PendingKey::MatchReplaceTo(from), Key::Character(to)) => {
                editor_replace_surround(editor, from, to)
            }
            (PendingKey::InsertLineAbove, Key::Character(' ')) => {
                editor_insert_blank_lines(editor, false)
            }
            (PendingKey::InsertLineBelow, Key::Character(' ')) => {
                editor_insert_blank_lines(editor, true)
            }
            (PendingKey::InsertLineAbove, Key::Character('g')) => {
                editor_goto_git_change(editor, false)
            }
            (PendingKey::InsertLineBelow, Key::Character('g')) => {
                editor_goto_git_change(editor, true)
            }
            (PendingKey::InsertLineAbove, Key::Character('f')) => {
                editor_goto_function(editor, false)
            }
            (PendingKey::InsertLineBelow, Key::Character('f')) => {
                editor_goto_function(editor, true)
            }
            (PendingKey::InsertLineAbove, Key::Character('d')) => {
                editor_next_diagnostic(editor, false)
            }
            (PendingKey::InsertLineBelow, Key::Character('d')) => {
                editor_next_diagnostic(editor, true)
            }
            (PendingKey::InsertLineAbove, Key::Character('a')) => {
                editor_goto_argument(editor, false)
            }
            (PendingKey::InsertLineBelow, Key::Character('a')) => {
                editor_goto_argument(editor, true)
            }
            _ => {}
        }
        return false;
    }

    match key {
        Key::Control(19) => {
            editor_save(editor);
            return false;
        }
        Key::Control(17) => return true,
        Key::Control(26) => {
            document_commit_transaction(editor_document_mut(editor));
            document_undo(editor_document_mut(editor));
            if editor.mode == Mode::Insert {
                document_begin_transaction(editor_document_mut(editor));
            }
            return false;
        }
        Key::Control(18) => {
            document_commit_transaction(editor_document_mut(editor));
            document_redo(editor_document_mut(editor));
            if editor.mode == Mode::Insert {
                document_begin_transaction(editor_document_mut(editor));
            }
            return false;
        }
        Key::Control(15) => {
            document_commit_transaction(editor_document_mut(editor));
            editor_jump(editor, false);
            return false;
        }
        Key::Control(9) => {
            editor_jump(editor, true);
            return false;
        }
        Key::Control(3) => {
            editor_toggle_line_comments(editor);
            return false;
        }
        _ => {}
    }

    match editor.mode {
        Mode::Insert => editor_handle_insert_key(editor, key),
        Mode::Normal | Mode::Select => editor_handle_command_key(editor, key),
    }
}

pub fn editor_save(editor: &mut Editor) {
    profiling::function_scope!();
    let current = editor.current;
    editor.status.clear();
    if document_write(&mut editor.documents[current], &mut editor.status) {
        editor.quit_warning = false;
        diagnostics_restart(&mut editor.diagnostics);
        editor_reindex_workspace(editor);
    }
}

fn document_write(document: &mut Document, status: &mut String) -> bool {
    profiling::function_scope!();
    let Some(path) = document.path.as_deref() else {
        status.push_str("no file name");
        return false;
    };
    let mut file = match std::fs::File::create(path) {
        Ok(file) => file,
        Err(error) => {
            write!(status, "write failed: {error}").unwrap();
            return false;
        }
    };
    if let Err(error) = buffer_write(&document.buffer, &mut file) {
        write!(status, "write failed: {error}").unwrap();
        return false;
    }
    if let Err(error) = file.flush() {
        write!(status, "write failed: {error}").unwrap();
        return false;
    }
    document.modified = false;
    write!(status, "wrote {} bytes", buffer_len(&document.buffer)).unwrap();
    true
}

fn editor_reindex_workspace(editor: &mut Editor) {
    profiling::function_scope!();
    if let Some(task) = editor.project_search_task.take() {
        task.cancel();
    }
    editor.project_search_task = Some(project_search_spawn(&editor.project));
    if !cfg!(test) {
        rust_method_index_restart(
            &mut editor.rust_methods,
            &editor.project.root,
            std::sync::Arc::clone(&editor.project.paths),
        );
    }
}

fn workspace_snapshot(root: std::path::PathBuf) -> WorkspaceSnapshot {
    profiling::function_scope!();
    let project = project_discover(root, usize::MAX).project;
    let temp = idno_std::mem().scratch().temp();
    let mut fingerprint_bytes = temp.vec(project.paths.len() * 32);
    for path in project.paths.iter() {
        fingerprint_bytes.extend_from_slice(path.as_os_str().as_encoded_bytes());
        fingerprint_bytes.push(0);
        let metadata = match std::fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        fingerprint_bytes.extend_from_slice(&metadata.len().to_ne_bytes());
        if let Ok(modified) = metadata.modified()
            && let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH)
        {
            fingerprint_bytes.extend_from_slice(&duration.as_secs().to_ne_bytes());
            fingerprint_bytes.extend_from_slice(&duration.subsec_nanos().to_ne_bytes());
        }
    }
    if let Some(ignore_file) = project.ignore_file.as_deref()
        && let Ok(metadata) = std::fs::metadata(ignore_file)
    {
        fingerprint_bytes.extend_from_slice(ignore_file.as_os_str().as_encoded_bytes());
        fingerprint_bytes.push(0);
        fingerprint_bytes.extend_from_slice(&metadata.len().to_ne_bytes());
        if let Ok(modified) = metadata.modified()
            && let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH)
        {
            fingerprint_bytes.extend_from_slice(&duration.as_secs().to_ne_bytes());
            fingerprint_bytes.extend_from_slice(&duration.subsec_nanos().to_ne_bytes());
        }
    }
    WorkspaceSnapshot {
        project,
        fingerprint: idno_std::utils::hash_rapid_bytes(&fingerprint_bytes),
    }
}

fn editor_poll_workspace_refresh(editor: &mut Editor) -> bool {
    profiling::function_scope!();
    let complete = editor
        .workspace_refresh_task
        .as_ref()
        .is_some_and(idno_std::micropool::OwnedTask::complete);
    if complete {
        let Some(task) = editor.workspace_refresh_task.take() else {
            return false;
        };
        let snapshot = task.join();
        editor.workspace_refresh_at = std::time::Instant::now() + std::time::Duration::from_secs(2);
        if editor.workspace_fingerprint == snapshot.fingerprint {
            return false;
        }
        editor.workspace_fingerprint = snapshot.fingerprint;
        editor.project = snapshot.project;
        editor.project_discovery = None;
        editor_reindex_workspace(editor);
        if editor.picker.is_some() {
            let mut picker = editor.picker.take().unwrap();
            picker_refresh(editor, &mut picker);
            picker.preview = None;
            picker_rebuild_preview(editor, &mut picker);
            editor.picker = Some(picker);
        }
        return true;
    }
    if editor.workspace_refresh_task.is_none()
        && editor.project_discovery.is_none()
        && !cfg!(test)
        && std::time::Instant::now() >= editor.workspace_refresh_at
    {
        let root = editor.project.root.clone();
        editor.workspace_refresh_task =
            Some(idno_std::threads().spawn_owned(move || workspace_snapshot(root)));
    }
    false
}

fn editor_finish_workspace_refresh(editor: &mut Editor) {
    profiling::function_scope!();
    let Some(task) = editor.workspace_refresh_task.take() else {
        return;
    };
    let snapshot = task.join();
    editor.workspace_refresh_at = std::time::Instant::now() + std::time::Duration::from_secs(2);
    if editor.workspace_fingerprint == snapshot.fingerprint {
        return;
    }
    editor.workspace_fingerprint = snapshot.fingerprint;
    editor.project = snapshot.project;
    editor.project_discovery = None;
    editor_reindex_workspace(editor);
}

fn editor_save_all(editor: &mut Editor) -> bool {
    profiling::function_scope!();
    editor.status.clear();
    let mut written = 0;
    for document in &mut editor.documents {
        if !document.modified {
            continue;
        }
        editor.status.clear();
        if !document_write(document, &mut editor.status) {
            return false;
        }
        written += 1;
    }
    editor.status.clear();
    write!(&mut editor.status, "wrote {written} buffer(s)").unwrap();
    editor.quit_warning = false;
    diagnostics_restart(&mut editor.diagnostics);
    if written > 0 {
        editor_reindex_workspace(editor);
    }
    true
}

pub fn document_undo(document: &mut Document) {
    profiling::function_scope!();
    let Some(transaction) = document.undo.pop() else {
        return;
    };
    git_gutter_invalidate(&mut document.git_gutter);
    for edit in transaction.edits.iter().rev() {
        let temp = idno_std::mem().scratch().temp();
        let mut byte_edits = temp.vec(edit.atoms.len());
        let mut replacements = temp.vec(edit.atoms.len());
        for atom in &edit.atoms {
            byte_edits.push((
                atom.after_start,
                atom.after_start + atom.inserted.len(),
                atom.deleted.len(),
            ));
            replacements.push((
                atom.after_start,
                atom.after_start + atom.inserted.len(),
                atom.deleted.clone(),
            ));
        }
        syntax_highlighting_invalidate_edits(&mut document.syntax, &byte_edits);
        code_index_invalidate_edits(&mut document.code_index, &byte_edits);
        let mut previous_line_starts = temp.vec(document.buffer.line_starts.len());
        buffer_replace_ranges(
            &mut document.buffer,
            &replacements,
            &edit.deleted_bytes,
            &mut previous_line_starts,
        );
        document_set_selections(document, &edit.before);
    }
    document.redo.push(transaction);
    document.modified = true;
}

pub fn document_redo(document: &mut Document) {
    profiling::function_scope!();
    let Some(transaction) = document.redo.pop() else {
        return;
    };
    git_gutter_invalidate(&mut document.git_gutter);
    for edit in &transaction.edits {
        let temp = idno_std::mem().scratch().temp();
        let mut byte_edits = temp.vec(edit.atoms.len());
        let mut replacements = temp.vec(edit.atoms.len());
        for atom in &edit.atoms {
            byte_edits.push((
                atom.before_start,
                atom.before_start + atom.deleted.len(),
                atom.inserted.len(),
            ));
            replacements.push((
                atom.before_start,
                atom.before_start + atom.deleted.len(),
                atom.inserted.clone(),
            ));
        }
        syntax_highlighting_invalidate_edits(&mut document.syntax, &byte_edits);
        code_index_invalidate_edits(&mut document.code_index, &byte_edits);
        let mut previous_line_starts = temp.vec(document.buffer.line_starts.len());
        buffer_replace_ranges(
            &mut document.buffer,
            &replacements,
            &edit.inserted_bytes,
            &mut previous_line_starts,
        );
        document_set_selections(document, &edit.after);
    }
    document.undo.push(transaction);
    document.modified = true;
}

pub fn document_begin_transaction(document: &mut Document) {
    if document.active_transaction.is_none() {
        document.active_transaction = Some(EditTransaction { edits: Vec::new() });
    }
}

pub fn document_commit_transaction(document: &mut Document) {
    let Some(transaction) = document.active_transaction.take() else {
        return;
    };
    if transaction.edits.is_empty() {
        return;
    }
    document.undo.push(transaction);
}

pub fn document_selections(
    document: &Document,
    selections: &mut Vec<SelectionState, impl Allocator>,
) {
    selections.clear();
    selections.reserve(document.secondary_selections.len() + 1);
    selections.extend_from_slice(&document.secondary_selections);
    selections.push(SelectionState {
        cursor: document.cursor,
        anchor: document.anchor,
    });
}

pub fn document_set_selections(document: &mut Document, selections: &[SelectionState]) {
    let Some((&primary, secondary)) = selections.split_last() else {
        return;
    };
    document.cursor = primary.cursor;
    document.anchor = primary.anchor;
    document.secondary_selections.clear();
    document.secondary_selections.extend_from_slice(secondary);
}

pub fn document_replace_ranges(
    document: &mut Document,
    replacements: &mut Vec<Replacement, impl Allocator>,
    inserted_bytes: &[u8],
    requested_after: Option<&[SelectionState]>,
) {
    profiling::function_scope!();
    if replacements.is_empty() {
        return;
    }
    replacements.sort_unstable_by_key(|replacement| replacement.start);
    replacements.dedup_by(|right, left| right.start == left.start && right.end == left.end);
    let temp = idno_std::mem().scratch().temp();
    let mut byte_edits = temp.vec(replacements.len());
    let mut line_edits = temp.vec(replacements.len());
    for replacement in replacements.iter() {
        let start_line = buffer_line_and_column(&document.buffer, replacement.start).0;
        let end_line = buffer_line_and_column(&document.buffer, replacement.end).0;
        let inserted_lines = inserted_bytes[replacement.inserted.clone()]
            .iter()
            .filter(|&&byte| byte == b'\n')
            .count();
        byte_edits.push((
            replacement.start,
            replacement.end,
            replacement.inserted.len(),
        ));
        line_edits.push((start_line, end_line, inserted_lines));
    }
    syntax_highlighting_invalidate_edits(&mut document.syntax, &byte_edits);
    code_index_invalidate_edits(&mut document.code_index, &byte_edits);
    git_gutter_invalidate(&mut document.git_gutter);
    git_gutter_adjust_edits(&mut document.git_gutter, &line_edits);
    let mut before = Vec::with_capacity(document.secondary_selections.len() + 1);
    document_selections(document, &mut before);
    let retained_insert_selection = document.active_transaction.is_some()
        && !document.insertion_points.is_empty()
        && requested_after.is_none();
    let mut retained_selection_bounds = temp.vec(before.len());
    let mut transformed_insertion_points = temp.vec(document.insertion_points.len());
    if retained_insert_selection {
        for selection in &before {
            retained_selection_bounds.push((
                selection.anchor.min(selection.cursor),
                buffer_next_char(&document.buffer, selection.anchor.max(selection.cursor)),
                selection.anchor <= selection.cursor,
                selection.anchor != selection.cursor,
            ));
        }
        for &position in &document.insertion_points {
            transformed_insertion_points.push(offset_after_replacements(
                position,
                true,
                replacements,
            ));
        }
    }
    let mut atoms = Vec::with_capacity(replacements.len());
    let deleted_capacity = replacements
        .iter()
        .map(|replacement| replacement.end - replacement.start)
        .sum();
    let mut deleted_bytes = Vec::with_capacity(deleted_capacity);
    let mut shift: isize = 0;
    for replacement in replacements.iter() {
        let after_start = (replacement.start as isize + shift) as usize;
        let deleted_start = deleted_bytes.len();
        buffer_append_range(
            &document.buffer,
            replacement.start,
            replacement.end,
            &mut deleted_bytes,
        );
        let deleted_end = deleted_bytes.len();
        shift +=
            replacement.inserted.len() as isize - (replacement.end - replacement.start) as isize;
        atoms.push(EditAtom {
            before_start: replacement.start,
            after_start,
            deleted: deleted_start..deleted_end,
            inserted: replacement.inserted.clone(),
        });
    }
    let mut buffer_replacements = temp.vec(replacements.len());
    buffer_replacements.extend(replacements.iter().map(|replacement| {
        (
            replacement.start,
            replacement.end,
            replacement.inserted.clone(),
        )
    }));
    let mut previous_line_starts = temp.vec(document.buffer.line_starts.len());
    buffer_replace_ranges(
        &mut document.buffer,
        &buffer_replacements,
        inserted_bytes,
        &mut previous_line_starts,
    );
    let mut after = Vec::with_capacity(before.len().max(atoms.len()));
    if let Some(requested_after) = requested_after {
        after.extend_from_slice(requested_after);
    } else if retained_insert_selection {
        for (selection_index, &(start, end, forward, extended)) in
            retained_selection_bounds.iter().enumerate()
        {
            if !extended {
                let position = transformed_insertion_points[selection_index];
                after.push(SelectionState {
                    cursor: position,
                    anchor: position,
                });
                continue;
            }
            let new_start = offset_after_replacements(start, false, replacements);
            let new_end = offset_after_replacements(end, true, replacements);
            let new_head = if new_end > new_start {
                buffer_previous_char(&document.buffer, new_end)
            } else {
                new_start
            };
            after.push(if forward {
                SelectionState {
                    anchor: new_start,
                    cursor: new_head,
                }
            } else {
                SelectionState {
                    anchor: new_head,
                    cursor: new_start,
                }
            });
        }
    } else {
        for atom in &atoms {
            let cursor = atom.after_start + atom.inserted.len();
            after.push(SelectionState {
                cursor,
                anchor: cursor,
            });
        }
    }
    document_set_selections(document, &after);
    if retained_insert_selection {
        document.insertion_points.clear();
        document
            .insertion_points
            .extend_from_slice(&transformed_insertion_points);
    }
    let edit = Edit {
        atoms,
        deleted_bytes,
        inserted_bytes: inserted_bytes.to_vec(),
        before,
        after,
    };
    if let Some(transaction) = &mut document.active_transaction {
        transaction.edits.push(edit);
    } else {
        document.undo.push(EditTransaction { edits: vec![edit] });
    }
    document.redo.clear();
    document.modified = true;
}

fn offset_after_replacements(
    position: usize,
    right_bias: bool,
    replacements: &[Replacement],
) -> usize {
    let mut shift = 0isize;
    for replacement in replacements {
        if position < replacement.start {
            break;
        }
        if position > replacement.end
            || (position == replacement.end && replacement.start != replacement.end)
        {
            shift += replacement.inserted.len() as isize
                - (replacement.end - replacement.start) as isize;
        } else if position == replacement.start && replacement.start == replacement.end {
            if right_bias {
                shift += replacement.inserted.len() as isize;
            }
        } else {
            return (replacement.start as isize
                + shift
                + if right_bias {
                    replacement.inserted.len() as isize
                } else {
                    0
                }) as usize;
        }
    }
    (position as isize + shift) as usize
}

pub fn editor_open_picker(editor: &mut Editor, kind: PickerKind) {
    let mut picker = Picker {
        kind,
        query: String::new(),
        matches: Vec::new(),
        selected: 0,
        first_visible: 0,
        tab_cycling: false,
        original_theme: editor.theme,
        theme_preview: false,
        search_query: String::new(),
        search_candidates: Vec::new(),
        search_candidate_position: 0,
        search_scan_position: 0,
        search_complete: kind != PickerKind::SearchProject,
        search_seen: Vec::new(),
        search_ranked: Vec::new(),
        symbol_corpus: SearchCorpus {
            bytes: Vec::new(),
            lines: Vec::new(),
            identifiers: Vec::new(),
            symbols: Vec::new(),
        },
        preview_corpus: SearchCorpus {
            bytes: Vec::new(),
            lines: Vec::new(),
            identifiers: Vec::new(),
            symbols: Vec::new(),
        },
        symbol_candidates: Vec::new(),
        rust_symbol_candidates: Vec::new(),
        reference_targets: Vec::new(),
        diagnostic_candidates: Vec::new(),
        preview: None,
        preview_load_key: None,
        preview_load_task: None,
    };
    if kind == PickerKind::DocumentSymbols {
        search_corpus_index_document(editor_document(editor), &mut picker.symbol_corpus);
        symbol_candidates_collect(&picker.symbol_corpus, None, &mut picker.symbol_candidates);
    } else if kind == PickerKind::WorkspaceSymbols {
        if rust_method_index_pending(&editor.rust_methods) {
            editor
                .status
                .push_str("workspace symbol index is still loading");
            return;
        }
        picker
            .rust_symbol_candidates
            .extend(0..editor.rust_methods.corpus.symbols.len());
    } else if matches!(
        kind,
        PickerKind::DocumentDiagnostics | PickerKind::WorkspaceDiagnostics
    ) {
        if diagnostics_pending(&editor.diagnostics) && editor.diagnostics.published.is_empty() {
            editor
                .status
                .push_str("compiler diagnostics are still loading");
            return;
        }
        let current_path = editor_document(editor)
            .path
            .as_ref()
            .map(|path| std::fs::canonicalize(path).unwrap_or_else(|_| path.clone()));
        for (diagnostic, item) in editor.diagnostics.published.iter().zip(0..) {
            if kind == PickerKind::WorkspaceDiagnostics
                || current_path.as_ref().is_some_and(|path| {
                    std::fs::canonicalize(&diagnostic.path)
                        .unwrap_or_else(|_| diagnostic.path.clone())
                        == *path
                })
            {
                picker.diagnostic_candidates.push(item);
            }
        }
    }
    picker_refresh(editor, &mut picker);
    picker_rebuild_preview(editor, &mut picker);
    editor.picker = Some(picker);
}

pub fn picker_refresh(editor: &Editor, picker: &mut Picker) {
    profiling::function_scope!();
    match picker.kind {
        PickerKind::Files => {
            if picker.query.is_empty() {
                picker.matches.clear();
                picker.search_ranked.clear();
            } else {
                let temp = idno_std::mem().scratch().temp();
                let mut labels = temp.vec(editor.project.labels.len());
                labels.extend(editor.project.labels.iter().map(String::as_str));
                fuzzy_rank(&picker.query, &labels, &mut picker.matches);
                picker.search_ranked.clear();
                picker
                    .search_ranked
                    .extend(picker.matches.iter().take(512).copied());
            }
        }
        PickerKind::Commands => {
            if let Some(argument) = command_theme_argument(&picker.query) {
                fuzzy_rank(argument, &THEME_NAMES, &mut picker.matches);
            } else if let Some(argument) = command_file_argument(&picker.query) {
                let temp = idno_std::mem().scratch().temp();
                let mut labels = temp.vec(editor.project.labels.len());
                labels.extend(editor.project.labels.iter().map(String::as_str));
                fuzzy_rank(argument, &labels, &mut picker.matches);
            } else {
                let command = picker.query.split_ascii_whitespace().next().unwrap_or("");
                fuzzy_rank(command, &COMMANDS, &mut picker.matches);
            }
        }
        PickerKind::SearchProject => {
            picker_project_search_begin(editor, picker);
        }
        PickerKind::DocumentSymbols => {
            let temp = idno_std::mem().scratch().temp();
            let mut labels = temp.vec(picker.symbol_candidates.len());
            for &line_index in &picker.symbol_candidates {
                let line = picker.symbol_corpus.lines[line_index];
                let source = &picker.symbol_corpus.bytes
                    [line.text_start as usize..line.display_end as usize];
                let Some((start, end)) = rust_symbol_name_range(source) else {
                    labels.push("");
                    continue;
                };
                labels.push(std::str::from_utf8(&source[start..end]).unwrap_or(""));
            }
            fuzzy_rank(&picker.query, &labels, &mut picker.matches);
        }
        PickerKind::WorkspaceSymbols => {
            let temp = idno_std::mem().scratch().temp();
            let mut labels = temp.vec(picker.rust_symbol_candidates.len());
            for &symbol in &picker.rust_symbol_candidates {
                labels.push(rust_symbol_name(&editor.rust_methods.corpus, symbol));
            }
            fuzzy_rank(&picker.query, &labels, &mut picker.matches);
        }
        PickerKind::References => {
            if picker.query.is_empty() {
                picker.matches.clear();
                picker.matches.extend(
                    (0..picker.reference_targets.len()).map(|item| FuzzyMatch { item, score: 0 }),
                );
            } else {
                let temp = idno_std::mem().scratch().temp();
                let mut labels = temp.vec(picker.symbol_corpus.lines.len());
                for line in &picker.symbol_corpus.lines {
                    labels.push(
                        std::str::from_utf8(
                            &picker.symbol_corpus.bytes
                                [line.display_start as usize..line.display_end as usize],
                        )
                        .unwrap_or(""),
                    );
                }
                fuzzy_rank(&picker.query, &labels, &mut picker.matches);
            }
        }
        PickerKind::DocumentDiagnostics | PickerKind::WorkspaceDiagnostics => {
            if picker.query.is_empty() {
                picker.matches.clear();
                picker.matches.extend(
                    (0..picker.diagnostic_candidates.len())
                        .map(|item| FuzzyMatch { item, score: 0 }),
                );
            } else {
                let temp = idno_std::mem().scratch().temp();
                let mut labels = temp.vec(picker.diagnostic_candidates.len());
                for &diagnostic in &picker.diagnostic_candidates {
                    labels.push(editor.diagnostics.published[diagnostic].display.as_str());
                }
                fuzzy_rank(&picker.query, &labels, &mut picker.matches);
                picker.matches.sort_unstable_by(|left, right| {
                    let left_diagnostic = picker.diagnostic_candidates[left.item];
                    let right_diagnostic = picker.diagnostic_candidates[right.item];
                    editor.diagnostics.published[left_diagnostic]
                        .severity
                        .cmp(&editor.diagnostics.published[right_diagnostic].severity)
                        .then_with(|| right.score.cmp(&left.score))
                });
            }
        }
    }
    picker.selected = picker
        .selected
        .min(picker_visible_len(editor, picker).saturating_sub(1));
    picker_ensure_selected_visible(picker, picker_result_rows(editor));
}

fn editor_handle_picker_key(editor: &mut Editor, key: Key) -> bool {
    match key {
        Key::Escape => {
            let mut picker = editor.picker.take().unwrap();
            if picker.theme_preview {
                editor.theme = picker.original_theme;
            }
            if let Some(task) = picker.preview_load_task.take() {
                task.cancel();
            }
        }
        Key::Enter => {
            editor_accept_picker(editor);
            if editor.quit_requested {
                return true;
            }
        }
        Key::Up | Key::Control(16) => {
            let rows = picker_result_rows(editor);
            let picker = editor.picker.as_mut().unwrap();
            picker.selected = picker.selected.saturating_sub(1);
            picker_ensure_selected_visible(picker, rows);
        }
        Key::Down | Key::Control(14) => {
            let visible_len = picker_visible_len(editor, editor.picker.as_ref().unwrap());
            let rows = picker_result_rows(editor);
            let picker = editor.picker.as_mut().unwrap();
            picker.selected = (picker.selected + 1).min(visible_len.saturating_sub(1));
            picker_ensure_selected_visible(picker, rows);
        }
        Key::Tab => {
            let mut picker = editor.picker.take().unwrap();
            picker_tab(editor, &mut picker, true);
            editor.picker = Some(picker);
        }
        Key::BackTab => {
            let mut picker = editor.picker.take().unwrap();
            picker_tab(editor, &mut picker, false);
            editor.picker = Some(picker);
        }
        Key::Backspace => {
            let mut picker = editor.picker.take().unwrap();
            picker_restore_preview_theme(editor, &mut picker);
            picker.tab_cycling = false;
            picker.query.pop();
            picker_refresh(editor, &mut picker);
            editor.picker = Some(picker);
        }
        Key::Character(character) => {
            let mut picker = editor.picker.take().unwrap();
            picker_restore_preview_theme(editor, &mut picker);
            picker.tab_cycling = false;
            if character == ' '
                && picker.kind == PickerKind::Commands
                && !picker.query.contains(char::is_whitespace)
            {
                picker_complete(editor, &mut picker);
                if !picker.query.ends_with(' ') {
                    picker.query.push(' ');
                }
            } else {
                picker.query.push(character);
            }
            picker_refresh(editor, &mut picker);
            editor.picker = Some(picker);
        }
        _ => {}
    }
    if let Some(mut picker) = editor.picker.take() {
        picker_rebuild_preview(editor, &mut picker);
        editor.picker = Some(picker);
    }
    false
}

fn picker_tab(editor: &mut Editor, picker: &mut Picker, forward: bool) {
    profiling::function_scope!();
    let visible_len = picker_visible_len(editor, picker);
    if visible_len == 0 {
        return;
    }
    let completes_argument = picker.kind == PickerKind::Commands
        && (command_theme_argument(&picker.query).is_some()
            || command_file_argument(&picker.query).is_some());
    if picker.kind != PickerKind::Commands {
        picker.selected = picker_next_selection(picker.selected, visible_len, forward);
        picker_ensure_selected_visible(picker, picker_result_rows(editor));
        return;
    }
    if completes_argument {
        if picker.tab_cycling {
            picker.selected = picker_next_selection(picker.selected, visible_len, forward);
        } else if !forward {
            picker.selected = picker_next_selection(picker.selected, visible_len, false);
        }
        picker.tab_cycling = true;
        picker_complete(editor, picker);
        picker_preview_theme(editor, picker);
        picker_ensure_selected_visible(picker, picker_result_rows(editor));
        return;
    }
    picker.selected = picker_next_selection(picker.selected, visible_len, forward);
    picker.tab_cycling = true;
    picker_ensure_selected_visible(picker, picker_result_rows(editor));
}

fn picker_result_rows(editor: &Editor) -> usize {
    let margin = (editor.terminal_height / 10).clamp(1, 4);
    editor
        .terminal_height
        .saturating_sub(margin * 2)
        .max(1)
        .saturating_sub(4)
}

fn picker_ensure_selected_visible(picker: &mut Picker, rows: usize) {
    if rows == 0 || picker.selected < picker.first_visible {
        picker.first_visible = picker.selected;
    } else if picker.selected >= picker.first_visible + rows {
        picker.first_visible = picker.selected + 1 - rows;
    }
}

fn picker_next_selection(selected: usize, length: usize, forward: bool) -> usize {
    if forward {
        (selected + 1) % length
    } else if selected == 0 {
        length - 1
    } else {
        selected - 1
    }
}

fn picker_preview_theme(editor: &mut Editor, picker: &mut Picker) {
    if command_theme_argument(&picker.query).is_none() {
        return;
    }
    let Some(found) = picker.matches.get(picker.selected) else {
        return;
    };
    if found.item >= THEMES.len() {
        return;
    }
    editor.theme = found.item;
    picker.theme_preview = true;
}

fn picker_restore_preview_theme(editor: &mut Editor, picker: &mut Picker) {
    if picker.theme_preview {
        editor.theme = picker.original_theme;
        picker.theme_preview = false;
    }
}

fn editor_accept_picker(editor: &mut Editor) {
    let Some(mut picker) = editor.picker.take() else {
        return;
    };
    if let Some(task) = picker.preview_load_task.take() {
        task.cancel();
    }
    if picker.kind == PickerKind::Commands && command_file_argument(&picker.query).is_some() {
        editor_execute_command(editor, &picker.query);
        return;
    }
    let item = if picker.kind == PickerKind::Files && picker.query.is_empty() {
        (picker.selected < editor.project.paths.len()).then_some(picker.selected)
    } else if picker.kind == PickerKind::Files {
        picker
            .search_ranked
            .get(picker.selected)
            .map(|found| found.item)
    } else if picker.kind == PickerKind::SearchProject {
        picker
            .search_ranked
            .get(picker.selected)
            .map(|found| found.item)
    } else {
        picker.matches.get(picker.selected).map(|found| found.item)
    };
    let Some(item) = item else {
        if picker.kind == PickerKind::Commands {
            editor_execute_command(editor, &picker.query);
        }
        return;
    };
    match picker.kind {
        PickerKind::Files => editor_open_project_file(editor, item),
        PickerKind::Commands => {
            if command_theme_argument(&picker.query).is_some() {
                editor_set_theme(editor, item);
            } else if picker.query.contains(char::is_whitespace) {
                editor_execute_command(editor, &picker.query);
            } else {
                editor_execute_command(editor, COMMANDS[item]);
            }
        }
        PickerKind::SearchProject => editor_accept_project_search(editor, &picker, item),
        PickerKind::DocumentSymbols => {
            let line_index = picker.symbol_candidates[item];
            let line = picker.symbol_corpus.lines[line_index];
            let source =
                &picker.symbol_corpus.bytes[line.text_start as usize..line.display_end as usize];
            let Some((name_start, name_end)) = rust_symbol_name_range(source) else {
                return;
            };
            editor_select_symbol(
                editor,
                line.file_offset as usize + name_start,
                line.file_offset as usize + name_end,
            );
        }
        PickerKind::WorkspaceSymbols => {
            let Some(&symbol) = picker.rust_symbol_candidates.get(item) else {
                return;
            };
            let Some(path) = rust_symbol_path(&editor.rust_methods.corpus, symbol) else {
                return;
            };
            let path = path.to_path_buf();
            let definition = editor.rust_methods.corpus.symbols[symbol];
            editor_navigate_to_symbol(
                editor,
                &path,
                definition.position as usize,
                definition.end as usize,
            );
        }
        PickerKind::References => {
            let Some(target) = picker.reference_targets.get(item).copied() else {
                return;
            };
            if target.project_file == u32::MAX {
                editor_select_symbol(editor, target.start as usize, target.end as usize);
            } else {
                let Some(path) = editor
                    .project
                    .paths
                    .get(target.project_file as usize)
                    .cloned()
                else {
                    return;
                };
                editor_navigate_to_symbol(
                    editor,
                    &path,
                    target.start as usize,
                    target.end as usize,
                );
            }
        }
        PickerKind::DocumentDiagnostics | PickerKind::WorkspaceDiagnostics => {
            let Some(&diagnostic) = picker.diagnostic_candidates.get(item) else {
                return;
            };
            editor_goto_diagnostic(editor, diagnostic);
        }
    }
}

fn command_theme_argument(query: &str) -> Option<&str> {
    let Some((command, argument)) = query.split_once(char::is_whitespace) else {
        return None;
    };
    if matches!(command, "theme" | "t") {
        Some(argument.trim_start())
    } else {
        None
    }
}

fn command_file_argument(query: &str) -> Option<&str> {
    let Some((command, argument)) = query.split_once(char::is_whitespace) else {
        return None;
    };
    if matches!(
        command,
        "write"
            | "w"
            | "write!"
            | "w!"
            | "write-quit"
            | "wq"
            | "write-quit!"
            | "wq!"
            | "open"
            | "o"
            | "edit"
            | "e"
    ) {
        Some(argument.trim_start())
    } else {
        None
    }
}

fn picker_complete(editor: &Editor, picker: &mut Picker) {
    let Some(found) = picker.matches.get(picker.selected) else {
        return;
    };
    match picker.kind {
        PickerKind::Files
        | PickerKind::SearchProject
        | PickerKind::DocumentSymbols
        | PickerKind::WorkspaceSymbols
        | PickerKind::References
        | PickerKind::DocumentDiagnostics
        | PickerKind::WorkspaceDiagnostics => {}
        PickerKind::Commands => {
            if command_theme_argument(&picker.query).is_some() {
                let command_end = picker
                    .query
                    .find(char::is_whitespace)
                    .unwrap_or(picker.query.len());
                picker.query.truncate(command_end);
                picker.query.push(' ');
                picker.query.push_str(THEME_NAMES[found.item]);
            } else if command_file_argument(&picker.query).is_some() {
                let command_end = picker
                    .query
                    .find(char::is_whitespace)
                    .unwrap_or(picker.query.len());
                picker.query.truncate(command_end);
                picker.query.push(' ');
                picker.query.push_str(&editor.project.labels[found.item]);
            } else {
                picker.query.clear();
                picker.query.push_str(COMMANDS[found.item]);
                if matches!(picker.query.as_str(), "theme" | "t") {
                    picker.query.push(' ');
                }
            }
        }
    }
}

fn project_search_spawn(project: &ProjectFiles) -> idno_std::micropool::OwnedTask<SearchCorpus> {
    let paths = std::sync::Arc::clone(&project.paths);
    let labels = std::sync::Arc::clone(&project.labels);
    idno_std::threads().spawn_owned(move || project_search_index(&paths, &labels))
}

fn editor_poll_project_discovery(editor: &mut Editor) -> bool {
    profiling::function_scope!();
    let Some(mut discovery) = editor.project_discovery.take() else {
        return false;
    };
    let previous_files = editor.project.paths.len();
    let complete = project_discovery_step(
        &mut editor.project,
        &mut discovery,
        4096,
        std::time::Duration::from_millis(2),
    );
    let discovered_files = editor.project.paths.len();
    let file_picker_visible =
        editor.picker.as_ref().map(|picker| picker.kind) == Some(PickerKind::Files);
    if file_picker_visible && discovered_files > previous_files {
        let mut picker = editor.picker.take().unwrap();
        picker_extend_file_matches(editor, &mut picker, previous_files);
        editor.picker = Some(picker);
    }
    if complete {
        if let Some(task) = editor.project_search_task.take() {
            task.cancel();
        }
        if project_search_should_build_inline(&editor.project) {
            editor.project_search = Some(project_search_index(
                &editor.project.paths,
                &editor.project.labels,
            ));
        } else {
            editor.project_search_task = Some(project_search_spawn(&editor.project));
        }
        if let Some(task) = editor.rust_methods.task.take() {
            task.cancel();
        }
        if !cfg!(test) {
            editor.rust_methods = rust_method_index_start(
                &editor.project.root,
                std::sync::Arc::clone(&editor.project.paths),
            );
        }
        if editor.project_search.is_some() && editor.picker.is_some() {
            let mut picker = editor.picker.take().unwrap();
            picker.preview = None;
            picker_rebuild_preview(editor, &mut picker);
            editor.picker = Some(picker);
        }
    } else {
        editor.project_discovery = Some(discovery);
    }
    (file_picker_visible && discovered_files > previous_files) || complete
}

fn picker_extend_file_matches(editor: &Editor, picker: &mut Picker, start: usize) {
    profiling::function_scope!();
    if start >= editor.project.labels.len() {
        return;
    }
    if picker.query.is_empty() {
        return;
    }
    let temp = idno_std::mem().scratch().temp();
    let mut labels = temp.vec(editor.project.labels.len() - start);
    labels.extend(editor.project.labels[start..].iter().map(String::as_str));
    let mut ranked = temp.vec(labels.len());
    fuzzy_rank(&picker.query, &labels, &mut ranked);
    picker.matches.reserve(ranked.len());
    picker.search_ranked.reserve(ranked.len());
    for found in &ranked {
        let found = FuzzyMatch {
            item: start + found.item,
            score: found.score,
        };
        picker.matches.push(found);
        picker.search_ranked.push(found);
    }
    picker.search_ranked.sort_unstable_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| {
                editor.project.labels[left.item]
                    .len()
                    .cmp(&editor.project.labels[right.item].len())
            })
            .then_with(|| editor.project.labels[left.item].cmp(&editor.project.labels[right.item]))
    });
    picker.search_ranked.truncate(512);
}

fn project_search_should_build_inline(project: &ProjectFiles) -> bool {
    const INLINE_FILES: usize = 256;
    const INLINE_BYTES: u64 = 8 * 1024 * 1024;
    if project.paths.len() > INLINE_FILES {
        return false;
    }
    let mut bytes = 0;
    for path in project.paths.iter() {
        let metadata = match std::fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        bytes += metadata.len();
        if bytes > INLINE_BYTES {
            return false;
        }
    }
    true
}

fn editor_poll_project_search(editor: &mut Editor) -> bool {
    profiling::function_scope!();
    let complete = editor
        .project_search_task
        .as_ref()
        .is_some_and(idno_std::micropool::OwnedTask::complete);
    if !complete {
        return false;
    }
    let Some(task) = editor.project_search_task.take() else {
        return false;
    };
    match task.try_join() {
        Ok(corpus) => editor.project_search = Some(corpus),
        Err(task) => {
            editor.project_search_task = Some(task);
            return false;
        }
    }
    if editor.picker.is_some() {
        let mut picker = editor.picker.take().unwrap();
        if picker.kind == PickerKind::SearchProject {
            picker_refresh(editor, &mut picker);
        }
        picker.preview = None;
        picker_rebuild_preview(editor, &mut picker);
        editor.picker = Some(picker);
    }
    true
}

fn editor_finish_project_search(editor: &mut Editor) {
    profiling::function_scope!();
    while editor.project_discovery.is_some() {
        editor_poll_project_discovery(editor);
    }
    let Some(task) = editor.project_search_task.take() else {
        return;
    };
    editor.project_search = Some(task.join());
}

fn picker_project_search_begin(editor: &Editor, picker: &mut Picker) {
    profiling::function_scope!();
    let extending = picker.query.len() > picker.search_query.len()
        && picker.query.starts_with(&picker.search_query);
    let old_scan_position = picker.search_scan_position;
    picker.search_candidates.clear();
    picker
        .search_candidates
        .extend(picker.matches.iter().map(|found| found.item));
    if extending {
        picker.search_scan_position = old_scan_position;
    } else {
        picker.search_scan_position = 0;
    }
    picker.search_candidate_position = 0;
    picker.matches.clear();
    picker.search_ranked.clear();
    picker.search_query.clear();
    picker.search_query.push_str(&picker.query);
    if picker.query.is_empty() {
        picker.search_complete = true;
        return;
    }
    picker.search_complete = false;
    picker.search_seen.clear();
    if let Some(corpus) = editor.project_search.as_ref() {
        picker
            .search_seen
            .resize(corpus.lines.len().div_ceil(64), 0);
    }
    picker_project_search_step(editor, picker, 2048);
}

fn picker_visible_len(editor: &Editor, picker: &Picker) -> usize {
    if picker.kind == PickerKind::SearchProject {
        picker.search_ranked.len()
    } else if picker.kind == PickerKind::Files && picker.query.is_empty() {
        editor.project.labels.len()
    } else if picker.kind == PickerKind::Files {
        picker.search_ranked.len()
    } else {
        picker.matches.len()
    }
}

fn picker_project_search_step(editor: &Editor, picker: &mut Picker, budget: usize) -> bool {
    profiling::function_scope!();
    let Some(corpus) = editor.project_search.as_ref() else {
        return false;
    };
    if picker.search_complete {
        return false;
    }
    let temp = idno_std::mem().scratch().temp();
    let mut items = temp.vec(budget);
    while items.len() < budget && picker.search_candidate_position < picker.search_candidates.len()
    {
        let item = picker.search_candidates[picker.search_candidate_position];
        picker.search_candidate_position += 1;
        let word = item / 64;
        let bit = 1u64 << (item % 64);
        if word < picker.search_seen.len() && picker.search_seen[word] & bit == 0 {
            picker.search_seen[word] |= bit;
            items.push(item);
        }
    }
    while items.len() < budget && picker.search_scan_position < corpus.lines.len() {
        let item = picker.search_scan_position;
        picker.search_scan_position += 1;
        let word = item / 64;
        let bit = 1u64 << (item % 64);
        if picker.search_seen[word] & bit == 0 {
            picker.search_seen[word] |= bit;
            items.push(item);
        }
    }
    let mut labels = temp.vec(items.len());
    for &item in &items {
        let line = corpus.lines[item];
        let text = &corpus.bytes[line.text_start as usize..line.display_end as usize];
        labels.push(std::str::from_utf8(text).unwrap_or(""));
    }
    let mut ranked = temp.vec(items.len());
    fuzzy_rank(&picker.query, &labels, &mut ranked);
    picker.matches.reserve(ranked.len());
    picker.search_ranked.reserve(ranked.len());
    for found in &ranked {
        let found = FuzzyMatch {
            item: items[found.item],
            score: found.score,
        };
        picker.matches.push(found);
        picker.search_ranked.push(found);
    }
    picker.search_ranked.sort_unstable_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.item.cmp(&right.item))
    });
    picker.search_ranked.truncate(512);
    picker.search_complete = picker.search_candidate_position >= picker.search_candidates.len()
        && picker.search_scan_position >= corpus.lines.len();
    picker.selected = picker
        .selected
        .min(picker.search_ranked.len().saturating_sub(1));
    !items.is_empty()
}

fn editor_step_project_search(editor: &mut Editor) -> bool {
    let needs_step = editor
        .picker
        .as_ref()
        .is_some_and(|picker| picker.kind == PickerKind::SearchProject && !picker.search_complete);
    if !needs_step {
        return false;
    }
    let mut picker = editor.picker.take().unwrap();
    let changed = picker_project_search_step(editor, &mut picker, 4096);
    if changed {
        picker_rebuild_preview(editor, &mut picker);
    }
    editor.picker = Some(picker);
    changed
}

fn editor_background_work_pending(editor: &Editor) -> bool {
    editor.project_discovery.is_some()
        || editor.project_search_task.is_some()
        || !editor_document(editor).syntax.complete
        || !editor_document(editor).code_index.complete
        || git_gutter_pending(&editor_document(editor).git_gutter)
        || rust_method_index_pending(&editor.rust_methods)
        || editor.clipboard_copy_task.is_some()
        || editor.clipboard_paste_task.is_some()
        || diagnostics_pending(&editor.diagnostics)
        || editor.format_task.is_some()
        || editor
            .picker
            .as_ref()
            .is_some_and(|picker| picker.preview_load_task.is_some())
        || editor
            .picker
            .as_ref()
            .and_then(|picker| picker.preview.as_ref())
            .is_some_and(|preview| !preview.syntax.complete)
        || editor.picker.as_ref().is_some_and(|picker| {
            picker.kind == PickerKind::SearchProject && !picker.search_complete
        })
}

fn editor_incremental_work_pending(editor: &Editor) -> bool {
    editor.project_discovery.is_some()
        || !editor_document(editor).syntax.complete
        || !editor_document(editor).code_index.complete
        || editor
            .picker
            .as_ref()
            .and_then(|picker| picker.preview.as_ref())
            .is_some_and(|preview| !preview.syntax.complete)
        || editor.picker.as_ref().is_some_and(|picker| {
            picker.kind == PickerKind::SearchProject && !picker.search_complete
        })
}

fn editor_poll_rust_methods(editor: &mut Editor) -> bool {
    profiling::function_scope!();
    if !rust_method_index_poll(&mut editor.rust_methods) {
        return false;
    }
    editor_refresh_completion(editor);
    true
}

fn editor_step_syntax_highlighting(editor: &mut Editor) -> bool {
    profiling::function_scope!();
    let document = editor_document_mut(editor);
    syntax_highlighting_step(
        &document.buffer,
        &mut document.syntax,
        128 * 1024,
        std::time::Duration::from_micros(500),
    )
}

fn editor_step_picker_preview(editor: &mut Editor) -> bool {
    profiling::function_scope!();
    let Some(preview) = editor
        .picker
        .as_mut()
        .and_then(|picker| picker.preview.as_mut())
    else {
        return false;
    };
    syntax_highlighting_step(
        &preview.buffer,
        &mut preview.syntax,
        128 * 1024,
        std::time::Duration::from_micros(500),
    )
}

fn editor_poll_picker_preview_load(editor: &mut Editor) -> bool {
    profiling::function_scope!();
    let complete = editor
        .picker
        .as_ref()
        .and_then(|picker| picker.preview_load_task.as_ref())
        .is_some_and(idno_std::micropool::OwnedTask::complete);
    if !complete {
        return false;
    }
    let mut picker = editor.picker.take().unwrap();
    let Some(task) = picker.preview_load_task.take() else {
        editor.picker = Some(picker);
        return false;
    };
    let loaded = match task.try_join() {
        Ok(loaded) => loaded,
        Err(task) => {
            picker.preview_load_task = Some(task);
            editor.picker = Some(picker);
            return false;
        }
    };
    if picker.preview_load_key == Some(loaded.key) && loaded.available {
        picker_preview_cache_set(
            &mut picker,
            loaded.key,
            Some(&loaded.path),
            &loaded.bytes,
            loaded.target_line,
            loaded.target_start..loaded.target_end,
            loaded.first_line_number,
        );
    }
    picker.preview_load_key = None;
    editor.picker = Some(picker);
    true
}

fn editor_step_code_index(editor: &mut Editor) -> bool {
    profiling::function_scope!();
    let document = editor_document_mut(editor);
    code_index_step(
        &document.buffer,
        &mut document.code_index,
        128 * 1024,
        std::time::Duration::from_micros(500),
    )
}

fn editor_step_git_gutter(editor: &mut Editor) -> bool {
    profiling::function_scope!();
    let document = editor_document_mut(editor);
    let polled = git_gutter_poll(&mut document.git_gutter);
    let stepped = git_gutter_step(
        &document.buffer,
        &mut document.git_gutter,
        128 * 1024,
        std::time::Duration::from_micros(500),
    );
    polled || stepped
}

fn search_corpus_index_document(document: &Document, corpus: &mut SearchCorpus) {
    profiling::function_scope!();
    corpus.bytes.clear();
    corpus.lines.clear();
    corpus.identifiers.clear();
    corpus.symbols.clear();
    let length = buffer_len(&document.buffer);
    let mut start = 0;
    let mut line_number = 1;
    while start <= length {
        let end = buffer_line_end(&document.buffer, start);
        let display_start = corpus.bytes.len();
        write!(&mut corpus.bytes, "{line_number}: ").unwrap();
        let text_start = corpus.bytes.len();
        buffer_append_range(&document.buffer, start, end, &mut corpus.bytes);
        let text_end = corpus.bytes.len();
        if text_end > u32::MAX as usize || start > u32::MAX as usize {
            corpus.bytes.truncate(display_start);
            break;
        }
        corpus.lines.push(SearchLine {
            project_file: u32::MAX,
            file_offset: start as u32,
            line_number,
            text_start: text_start as u32,
            display_start: display_start as u32,
            display_end: text_end as u32,
        });
        if end >= length {
            break;
        }
        start = end + 1;
        line_number += 1;
    }
}

fn symbol_candidates_collect(
    corpus: &SearchCorpus,
    workspace_labels: Option<(&[String], bool)>,
    candidates: &mut Vec<usize>,
) {
    profiling::function_scope!();
    candidates.clear();
    for (line_index, line) in corpus.lines.iter().enumerate() {
        if let Some((labels, rust_only)) = workspace_labels {
            let project_file = line.project_file as usize;
            if project_file >= labels.len() || (rust_only && !labels[project_file].ends_with(".rs"))
            {
                continue;
            }
        }
        let source = &corpus.bytes[line.text_start as usize..line.display_end as usize];
        if rust_symbol_name_range(source).is_some() {
            candidates.push(line_index);
        }
    }
}

fn rust_symbol_name_range(line: &[u8]) -> Option<(usize, usize)> {
    let mut position = 0;
    rust_skip_ascii_whitespace(line, &mut position);
    loop {
        let start = position;
        while position < line.len()
            && (line[position].is_ascii_alphanumeric() || line[position] == b'_')
        {
            position += 1;
        }
        let word = &line[start..position];
        if word == b"pub" {
            rust_skip_ascii_whitespace(line, &mut position);
            if position < line.len() && line[position] == b'(' {
                let mut depth = 1usize;
                position += 1;
                while position < line.len() && depth > 0 {
                    depth += usize::from(line[position] == b'(');
                    depth = depth.saturating_sub(usize::from(line[position] == b')'));
                    position += 1;
                }
                rust_skip_ascii_whitespace(line, &mut position);
            }
            continue;
        }
        if matches!(
            word,
            b"async" | b"const" | b"unsafe" | b"extern" | b"default"
        ) {
            rust_skip_ascii_whitespace(line, &mut position);
            if word == b"extern" && position < line.len() && line[position] == b'"' {
                position += 1;
                while position < line.len() && line[position] != b'"' {
                    position += 1;
                }
                position = (position + usize::from(position < line.len())).min(line.len());
                rust_skip_ascii_whitespace(line, &mut position);
            }
            continue;
        }
        if word == b"macro_rules" {
            if line.get(position) == Some(&b'!') {
                position += 1;
            }
            rust_skip_ascii_whitespace(line, &mut position);
            let name_start = position;
            while position < line.len()
                && (line[position].is_ascii_alphanumeric() || line[position] == b'_')
            {
                position += 1;
            }
            return (position > name_start).then_some((name_start, position));
        }
        if !matches!(
            word,
            b"fn" | b"struct" | b"enum" | b"trait" | b"type" | b"mod" | b"static" | b"let"
        ) {
            return None;
        }
        rust_skip_ascii_whitespace(line, &mut position);
        if word == b"let" && line.get(position..position + 4) == Some(b"mut ") {
            position += 4;
        }
        let name_start = position;
        while position < line.len()
            && (line[position].is_ascii_alphanumeric() || line[position] == b'_')
        {
            position += 1;
        }
        return (position > name_start).then_some((name_start, position));
    }
}

fn rust_skip_ascii_whitespace(line: &[u8], position: &mut usize) {
    while *position < line.len() && line[*position].is_ascii_whitespace() {
        *position += 1;
    }
}

fn project_search_index(paths: &[std::path::PathBuf], labels: &[String]) -> SearchCorpus {
    profiling::function_scope!();
    profiling::scope!("read_project_search_corpus");
    let mut corpus = SearchCorpus {
        bytes: Vec::with_capacity(256 * 1024),
        lines: Vec::with_capacity(4096),
        identifiers: Vec::with_capacity(16 * 1024),
        symbols: Vec::with_capacity(4096),
    };
    let temp = idno_std::mem().scratch().temp();
    let mut source = temp.vec(64 * 1024);
    let mut source_identifiers = temp.vec(8192);
    let mut read_bytes = [0; 64 * 1024];
    for (project_file, path) in paths.iter().enumerate() {
        source.clear();
        let mut file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(_) => continue,
        };
        let mut read_failed = false;
        loop {
            match file.read(&mut read_bytes) {
                Ok(0) => break,
                Ok(read) => {
                    if read_bytes[..read].contains(&0) {
                        read_failed = true;
                        break;
                    }
                    source.extend_from_slice(&read_bytes[..read]);
                }
                Err(_) => {
                    read_failed = true;
                    break;
                }
            }
        }
        if read_failed || std::str::from_utf8(&source).is_err() {
            continue;
        }
        let label = &labels[project_file];
        source_identifiers.clear();
        if label.ends_with(".rs") {
            rust_source_identifiers(&source, &mut source_identifiers);
        }
        let mut source_identifier = 0usize;
        let mut start = 0;
        let mut line_number = 1;
        while start <= source.len() {
            let end = source[start..]
                .iter()
                .position(|&byte| byte == b'\n')
                .map_or(source.len(), |offset| start + offset);
            let display_start = corpus.bytes.len();
            let text_start = display_start;
            corpus.bytes.extend_from_slice(&source[start..end]);
            let text_end = corpus.bytes.len();
            if text_end > u32::MAX as usize
                || start > u32::MAX as usize
                || project_file > u32::MAX as usize
            {
                corpus.bytes.truncate(display_start);
                break;
            }
            corpus.lines.push(SearchLine {
                project_file: project_file as u32,
                file_offset: start as u32,
                line_number,
                text_start: text_start as u32,
                display_start: display_start as u32,
                display_end: text_end as u32,
            });
            if label.ends_with(".rs")
                && let Some((name_start, name_end)) = rust_symbol_name_range(&source[start..end])
            {
                corpus.symbols.push(SearchSymbol {
                    name_start: (text_start + name_start) as u32,
                    name_end: (text_start + name_end) as u32,
                    project_file: project_file as u32,
                    start: (start + name_start) as u32,
                    end: (start + name_end) as u32,
                });
            }
            let line_index = corpus.lines.len() - 1;
            while source_identifier < source_identifiers.len()
                && (source_identifiers[source_identifier].start as usize) < end
            {
                let identifier = source_identifiers[source_identifier].clone();
                if identifier.start as usize >= start
                    && identifier.end as usize <= end
                    && line_index <= u32::MAX as usize
                {
                    let within_start = identifier.start as usize - start;
                    let within_end = identifier.end as usize - start;
                    corpus.identifiers.push(SearchIdentifier {
                        name_start: (text_start + within_start) as u32,
                        name_end: (text_start + within_end) as u32,
                        project_file: project_file as u32,
                        file_start: identifier.start,
                        file_end: identifier.end,
                        line: line_index as u32,
                    });
                }
                source_identifier += 1;
            }
            if end >= source.len() {
                break;
            }
            start = end + 1;
            line_number += 1;
        }
    }
    corpus.identifiers.sort_unstable_by(|left, right| {
        corpus.bytes[left.name_start as usize..left.name_end as usize]
            .cmp(&corpus.bytes[right.name_start as usize..right.name_end as usize])
            .then_with(|| left.project_file.cmp(&right.project_file))
            .then_with(|| left.file_start.cmp(&right.file_start))
    });
    corpus.symbols.sort_unstable_by(|left, right| {
        corpus.bytes[left.name_start as usize..left.name_end as usize]
            .cmp(&corpus.bytes[right.name_start as usize..right.name_end as usize])
            .then_with(|| left.project_file.cmp(&right.project_file))
            .then_with(|| left.start.cmp(&right.start))
    });
    corpus
}

fn editor_accept_project_search(editor: &mut Editor, picker: &Picker, item: usize) {
    let Some(corpus) = editor.project_search.as_ref() else {
        return;
    };
    let Some(line) = corpus.lines.get(item).copied() else {
        return;
    };
    let before = editor_location(editor);
    editor_open_project_file(editor, line.project_file as usize);
    let searched_project_file = line.project_file;
    editor.search_query.clear();
    editor.search_query.push_str(&picker.query);
    editor.search_matches.clear();
    for found in &picker.matches {
        let corpus = editor.project_search.as_ref().unwrap();
        let found_line = &corpus.lines[found.item];
        if found_line.project_file != searched_project_file {
            continue;
        }
        let found_text =
            &corpus.bytes[found_line.text_start as usize..found_line.display_end as usize];
        if let Some((start, end)) = fuzzy_subsequence_span(picker.query.as_bytes(), found_text) {
            let file_offset = found_line.file_offset as usize;
            let anchor = file_offset + start;
            editor.search_matches.push(SelectionState {
                anchor,
                cursor: end.saturating_sub(1) + file_offset,
            });
        }
    }
    editor
        .search_matches
        .sort_unstable_by_key(|selection| selection.anchor);
    let document = editor_document_mut(editor);
    let file_offset = line.file_offset as usize;
    let line_end = buffer_line_end(&document.buffer, file_offset);
    let start = file_offset.min(line_end);
    document.anchor = start;
    document.cursor = if line_end < buffer_len(&document.buffer) {
        line_end
    } else if line_end > start {
        buffer_previous_char(&document.buffer, line_end)
    } else {
        start
    };
    document.secondary_selections.clear();
    document.preferred_column = buffer_line_and_column(&document.buffer, document.cursor).1;
    editor.search_position = editor
        .search_matches
        .iter()
        .position(|selection| start <= selection.anchor && selection.anchor <= line_end)
        .unwrap_or(0);
    editor.mode = Mode::Normal;
    editor_center_view(editor);
    let after = editor_location(editor);
    editor_record_jump(editor, before, after);
}

fn editor_start_search(editor: &mut Editor, kind: SearchKind) {
    profiling::function_scope!();
    let current = editor.current;
    let document = &editor.documents[current];
    let mut original_selections = Vec::with_capacity(document.secondary_selections.len() + 1);
    document_selections(document, &mut original_selections);
    let mut corpus = SearchCorpus {
        bytes: Vec::with_capacity(buffer_len(&document.buffer)),
        lines: Vec::with_capacity(buffer_line_count(&document.buffer)),
        identifiers: Vec::new(),
        symbols: Vec::new(),
    };
    search_corpus_index_document(document, &mut corpus);
    editor.search = Some(SearchSession {
        kind,
        document: editor.current,
        query: String::new(),
        original_selections,
        matches: Vec::new(),
        selected: 0,
        corpus,
    });
}

fn editor_search_refresh(editor: &Editor, search: &mut SearchSession) {
    profiling::function_scope!();
    search.matches.clear();
    search.selected = 0;
    if search.query.is_empty() || search.document >= editor.documents.len() {
        return;
    }
    match search.kind {
        SearchKind::Document => {
            search_document_exact_matches(search);
            search
                .matches
                .sort_unstable_by_key(|selection| selection.anchor);
            let origin = search
                .original_selections
                .last()
                .map_or(0, |selection| selection.cursor);
            search.selected = search
                .matches
                .iter()
                .position(|selection| selection.anchor > origin)
                .unwrap_or(0);
        }
        SearchKind::Selection => {
            search_selection_exact_matches(&editor.documents[search.document], search)
        }
    }
}

fn search_document_exact_matches(search: &mut SearchSession) {
    profiling::function_scope!();
    let query = search.query.as_bytes();
    for line in &search.corpus.lines {
        let text = &search.corpus.bytes[line.text_start as usize..line.display_end as usize];
        let mut start = 0;
        while start + query.len() <= text.len() {
            let matched = query
                .iter()
                .enumerate()
                .all(|(offset, &query_byte)| fuzzy_byte_matches(query_byte, text[start + offset]));
            if matched {
                let file_offset = line.file_offset as usize;
                search.matches.push(SelectionState {
                    anchor: file_offset + start,
                    cursor: file_offset + start + query.len().saturating_sub(1),
                });
            }
            start += 1;
        }
    }
}

fn search_selection_exact_matches(document: &Document, search: &mut SearchSession) {
    profiling::function_scope!();
    let query = search.query.as_bytes();
    for selection in &search.original_selections {
        let start = selection.anchor.min(selection.cursor);
        let end = buffer_next_char(&document.buffer, selection.anchor.max(selection.cursor));
        let mut position = start;
        while position + query.len() <= end {
            let mut matched = true;
            for (offset, &query_byte) in query.iter().enumerate() {
                if !fuzzy_byte_matches(query_byte, buffer_byte(&document.buffer, position + offset))
                {
                    matched = false;
                    break;
                }
            }
            if matched {
                search.matches.push(SelectionState {
                    anchor: position,
                    cursor: position + query.len() - 1,
                });
                position += query.len();
            } else {
                position = buffer_next_char(&document.buffer, position);
            }
        }
    }
}

fn editor_handle_search_key(editor: &mut Editor, key: Key) -> bool {
    match key {
        Key::Escape => editor.search = None,
        Key::Enter => editor_commit_search(editor),
        Key::Up | Key::Control(16) => {
            let search = editor.search.as_mut().unwrap();
            search.selected = search.selected.saturating_sub(1);
        }
        Key::Down | Key::Control(14) | Key::Tab => {
            let search = editor.search.as_mut().unwrap();
            search.selected = (search.selected + 1).min(search.matches.len().saturating_sub(1));
        }
        Key::Backspace => {
            let mut search = editor.search.take().unwrap();
            search.query.pop();
            editor_search_refresh(editor, &mut search);
            editor.search = Some(search);
        }
        Key::Character(character) => {
            let mut search = editor.search.take().unwrap();
            search.query.push(character);
            editor_search_refresh(editor, &mut search);
            editor.search = Some(search);
        }
        _ => {}
    }
    false
}

fn editor_commit_search(editor: &mut Editor) {
    let Some(search) = editor.search.take() else {
        return;
    };
    if search.matches.is_empty() || search.document != editor.current {
        return;
    }
    match search.kind {
        SearchKind::Document => {
            let selection = search.matches[search.selected];
            let document = editor_document_mut(editor);
            document.anchor = selection.anchor;
            document.cursor = selection.cursor;
            document.secondary_selections.clear();
            editor.search_query = search.query;
            editor.search_matches = search.matches;
            editor
                .search_matches
                .sort_unstable_by_key(|selection| selection.anchor);
            editor.search_position = editor
                .search_matches
                .iter()
                .position(|candidate| candidate.anchor == selection.anchor)
                .unwrap_or(0);
        }
        SearchKind::Selection => {
            document_set_selections(editor_document_mut(editor), &search.matches);
        }
    }
}

fn editor_search_match(editor: &mut Editor, forward: bool) {
    if editor.search_matches.is_empty() {
        editor.status.push_str("no active search");
        return;
    }
    if forward {
        editor.search_position = (editor.search_position + 1) % editor.search_matches.len();
    } else {
        editor.search_position = editor
            .search_position
            .checked_sub(1)
            .unwrap_or(editor.search_matches.len() - 1);
    }
    let selection = editor.search_matches[editor.search_position];
    let document = editor_document_mut(editor);
    document.anchor = selection.anchor.min(buffer_len(&document.buffer));
    document.cursor = selection.cursor.min(buffer_len(&document.buffer));
    document.secondary_selections.clear();
    document.preferred_column = buffer_line_and_column(&document.buffer, document.cursor).1;
}

fn fuzzy_subsequence_span(query: &[u8], candidate: &[u8]) -> Option<(usize, usize)> {
    profiling::function_scope!();
    if query.is_empty() {
        return Some((0, 0));
    }
    if let Some(span) = fuzzy_subsequence_span_with_swap(query, candidate, usize::MAX) {
        return Some(span);
    }
    let mut best: Option<(usize, usize)> = None;
    for swap in 0..query.len().saturating_sub(1) {
        if query[swap].eq_ignore_ascii_case(&query[swap + 1]) {
            continue;
        }
        if let Some(span) = fuzzy_subsequence_span_with_swap(query, candidate, swap)
            && best.is_none_or(|best| span.1 - span.0 < best.1 - best.0)
        {
            best = Some(span);
        }
    }
    best
}

fn fuzzy_subsequence_span_with_swap(
    query: &[u8],
    candidate: &[u8],
    swap: usize,
) -> Option<(usize, usize)> {
    let mut best = None;
    let mut position = 0;
    while position < candidate.len() {
        let mut query_position = 0;
        while position < candidate.len() && query_position < query.len() {
            let expected = fuzzy_query_position_with_swap(query_position, swap);
            if fuzzy_byte_matches(query[expected], candidate[position]) {
                query_position += 1;
            }
            position += 1;
        }
        if query_position != query.len() {
            break;
        }

        let end = position;
        query_position = query.len();
        while position > 0 && query_position > 0 {
            position -= 1;
            let expected = fuzzy_query_position_with_swap(query_position - 1, swap);
            if fuzzy_byte_matches(query[expected], candidate[position]) {
                query_position -= 1;
            }
        }
        let start = position;
        if best.is_none_or(|(best_start, best_end)| end - start < best_end - best_start) {
            best = Some((start, end));
        }
        position = start + 1;
    }
    best
}

#[inline]
fn fuzzy_query_position_with_swap(query_position: usize, swap: usize) -> usize {
    if swap != usize::MAX && query_position == swap {
        query_position + 1
    } else if swap != usize::MAX && query_position == swap + 1 {
        query_position - 1
    } else {
        query_position
    }
}

pub fn editor_open_project_file(editor: &mut Editor, project_file: usize) {
    let Some(path) = editor.project.paths.get(project_file).cloned() else {
        return;
    };
    let Some(target) = editor_document_target(editor, path) else {
        return;
    };
    editor_switch_document(editor, target);
}

fn editor_document_target(editor: &mut Editor, path: std::path::PathBuf) -> Option<usize> {
    profiling::function_scope!();
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    if let Some(document) = editor.documents.iter().position(|document| {
        document.path.as_ref().is_some_and(|open| {
            std::fs::canonicalize(open).unwrap_or_else(|_| open.clone()) == path
        })
    }) {
        return Some(document);
    }
    match document_open(Some(path)) {
        Ok(document) => {
            editor.documents.push(document);
            Some(editor.documents.len() - 1)
        }
        Err(error) => {
            write!(&mut editor.status, "open failed: {error}").unwrap();
            None
        }
    }
}

pub fn editor_switch_document(editor: &mut Editor, target: usize) {
    if target == editor.current || target >= editor.documents.len() {
        return;
    }
    let before = editor_location(editor);
    editor_switch_document_state(editor, target);
    let after = editor_location(editor);
    editor_record_jump(editor, before, after);
}

fn editor_switch_document_state(editor: &mut Editor, target: usize) {
    if target == editor.current || target >= editor.documents.len() {
        return;
    }
    document_commit_transaction(editor_document_mut(editor));
    editor.last_accessed_document = Some(editor.current);
    editor.current = target;
    editor.mode = Mode::Normal;
}

pub fn editor_location(editor: &Editor) -> Jump {
    let document = editor_document(editor);
    Jump {
        document: editor.current,
        cursor: document.cursor,
        anchor: document.anchor,
        top_line: document.top_line,
    }
}

pub fn editor_record_jump(editor: &mut Editor, before: Jump, after: Jump) {
    if editor.jump_position + 1 < editor.jumps.len() {
        editor.jumps.truncate(editor.jump_position + 1);
    }
    if editor.jumps.last().copied() != Some(before) {
        editor.jumps.push(before);
    }
    if editor.jumps.last().copied() != Some(after) {
        editor.jumps.push(after);
    }
    editor.jump_position = editor.jumps.len().saturating_sub(1);
}

pub fn editor_jump(editor: &mut Editor, forward: bool) {
    if editor.jumps.is_empty() {
        return;
    }
    if forward {
        if editor.jump_position + 1 >= editor.jumps.len() {
            return;
        }
        editor.jump_position += 1;
    } else {
        if editor.jump_position == 0 {
            return;
        }
        editor.jump_position -= 1;
    }
    let jump = editor.jumps[editor.jump_position];
    if jump.document != editor.current {
        editor.last_accessed_document = Some(editor.current);
    }
    editor.current = jump.document;
    let document = editor_document_mut(editor);
    document.cursor = jump.cursor.min(buffer_len(&document.buffer));
    document.anchor = jump.anchor.min(buffer_len(&document.buffer));
    document.top_line = jump.top_line;
    document.preferred_column = buffer_line_and_column(&document.buffer, document.cursor).1;
    editor.mode = Mode::Normal;
}

pub fn editor_execute_command(editor: &mut Editor, input: &str) {
    profiling::function_scope!();
    let mut words = input.split_ascii_whitespace();
    let command = words.next().unwrap_or("");
    let argument = words.next();
    match command {
        "write" | "w" | "write!" | "w!" => {
            if let Some(path) = argument {
                editor_set_document_path(editor, path);
            }
            editor_save(editor);
        }
        "quit" | "q" => editor_request_quit(editor),
        "quit!" | "q!" | "quit-all!" | "qa!" => editor.quit_requested = true,
        "quit-all" | "qa" => editor_request_quit(editor),
        "write-quit" | "wq" | "write-quit!" | "wq!" => {
            if let Some(path) = argument {
                editor_set_document_path(editor, path);
            }
            editor_save(editor);
            if !editor_document(editor).modified {
                editor.quit_requested = true;
            }
        }
        "write-all" | "wa" | "write-all!" | "wa!" => {
            editor_save_all(editor);
        }
        "write-quit-all" | "wqa" | "xa" | "write-quit-all!" | "wqa!" | "xa!" => {
            if editor_save_all(editor) {
                editor.quit_requested = true;
            }
        }
        "open" | "o" | "edit" | "e" => match argument {
            Some(path) => editor_open_path(editor, path),
            None => editor_open_picker(editor, PickerKind::Files),
        },
        "new" | "n" => editor_new_document(editor),
        "buffer-close" | "bc" | "bclose" => editor_close_current_document(editor, false),
        "buffer-close!" | "bc!" | "bclose!" => editor_close_current_document(editor, true),
        "buffer-close-others" | "bco" => editor_close_other_documents(editor, false),
        "buffer-close-others!" | "bco!" => editor_close_other_documents(editor, true),
        "buffer-close-all" | "bca" => editor_close_all_documents(editor, false),
        "buffer-close-all!" | "bca!" => editor_close_all_documents(editor, true),
        "undo" => document_undo(editor_document_mut(editor)),
        "redo" => document_redo(editor_document_mut(editor)),
        "buffer-next" | "bn" => {
            editor_switch_document(editor, (editor.current + 1) % editor.documents.len())
        }
        "buffer-previous" | "bp" => editor_switch_document(
            editor,
            editor
                .current
                .checked_sub(1)
                .unwrap_or(editor.documents.len() - 1),
        ),
        "theme" | "t" => match argument {
            Some(name) => editor_set_theme_name(editor, name),
            None => write!(&mut editor.status, "theme {}", THEMES[editor.theme].name).unwrap(),
        },
        "reload-files" => {
            if let Some(task) = editor.project_search_task.take() {
                task.cancel();
            }
            let discovery = project_discover(editor.project.root.clone(), 1024);
            editor.project = discovery.project;
            editor.project_discovery = discovery.state;
            editor.project_search = None;
            if !discovery.complete {
                editor.project_search_task = Some(project_search_spawn(&editor.project));
            } else if project_search_should_build_inline(&editor.project) {
                editor.project_search = Some(project_search_index(
                    &editor.project.paths,
                    &editor.project.labels,
                ));
            } else {
                editor.project_search_task = Some(project_search_spawn(&editor.project));
            }
            editor.status.push_str("project files reloaded");
        }
        "reload" | "reload!" | "rl" | "rl!" => {
            editor_reload_document(editor);
        }
        "reload-all" | "reload-all!" | "rla" | "rla!" => editor_reload_all_documents(editor),
        "format" => editor_format_document(editor),
        "toggle-auto-indentation" => {
            let enabled = !editor.config.flags.contains(EditorFlags::AUTO_INDENTATION);
            editor
                .config
                .flags
                .set(EditorFlags::AUTO_INDENTATION, enabled);
        }
        "toggle-auto-indent-scopes" => {
            let enabled = !editor
                .config
                .flags
                .contains(EditorFlags::AUTO_INDENT_SCOPES);
            editor
                .config
                .flags
                .set(EditorFlags::AUTO_INDENT_SCOPES, enabled);
        }
        "toggle-auto-pairs" => {
            let enabled = !editor.config.flags.contains(EditorFlags::AUTO_PAIRS);
            editor.config.flags.set(EditorFlags::AUTO_PAIRS, enabled);
        }
        _ => write!(&mut editor.status, "unknown command: {command}").unwrap(),
    }
}

fn editor_reload_document(editor: &mut Editor) -> bool {
    profiling::function_scope!();
    document_commit_transaction(editor_document_mut(editor));
    let Some(path) = editor_document(editor).path.as_deref() else {
        editor
            .status
            .push_str("scratch buffer has no file to reload");
        return false;
    };
    let source = match std::fs::read(path) {
        Ok(source) => source,
        Err(error) => {
            write!(&mut editor.status, "reload failed: {error}").unwrap();
            return false;
        }
    };
    let unchanged = {
        let document = editor_document(editor);
        source.len() == buffer_len(&document.buffer)
            && source
                .iter()
                .enumerate()
                .all(|(position, &byte)| buffer_byte(&document.buffer, position) == byte)
    };
    if unchanged {
        editor_document_mut(editor).modified = false;
        editor.status.push_str("buffer already matches disk");
        return true;
    }
    let document = editor_document_mut(editor);
    let temp = idno_std::mem().scratch().temp();
    let mut replacements = temp.vec(1);
    replacements.push(Replacement {
        start: 0,
        end: buffer_len(&document.buffer),
        inserted: 0..source.len(),
    });
    let mut cursor = document.cursor.min(source.len());
    while cursor > 0 && cursor < source.len() && source[cursor] & 0b1100_0000 == 0b1000_0000 {
        cursor -= 1;
    }
    if cursor == source.len() && cursor > 0 && source[cursor - 1] != b'\n' {
        cursor -= 1;
        while cursor > 0 && source[cursor] & 0b1100_0000 == 0b1000_0000 {
            cursor -= 1;
        }
    }
    let after = [SelectionState {
        cursor,
        anchor: cursor,
    }];
    document_replace_ranges(document, &mut replacements, &source, Some(&after));
    document.modified = false;
    write!(&mut editor.status, "reloaded {} bytes", source.len()).unwrap();
    editor_reindex_workspace(editor);
    true
}

fn editor_reload_all_documents(editor: &mut Editor) {
    profiling::function_scope!();
    let current = editor.current;
    let mut reloaded = 0;
    for document in 0..editor.documents.len() {
        if editor.documents[document].path.is_none() {
            continue;
        }
        editor.current = document;
        reloaded += usize::from(editor_reload_document(editor));
    }
    editor.current = current.min(editor.documents.len().saturating_sub(1));
    editor.status.clear();
    write!(&mut editor.status, "reloaded {reloaded} buffer(s)").unwrap();
}

fn editor_format_document(editor: &mut Editor) {
    profiling::function_scope!();
    if editor.format_task.is_some() {
        editor.status.push_str("formatter is already running");
        return;
    }
    let document = editor_document(editor);
    let Some(path) = document.path.as_deref() else {
        editor.status.push_str("scratch buffer has no formatter");
        return;
    };
    let extension = path
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("");
    let (formatter, arguments): (&'static str, &'static [&'static str]) = match extension {
        "rs" => ("rustfmt", &["--emit", "stdout", "--edition", "2024"]),
        "c" | "cc" | "cpp" | "cxx" | "h" | "hh" | "hpp" | "hxx" => ("clang-format", &[]),
        "go" => ("gofmt", &[]),
        _ => {
            editor.status.push_str("no formatter for current file");
            return;
        }
    };
    let mut original = Vec::with_capacity(buffer_len(&document.buffer));
    buffer_append_range(
        &document.buffer,
        0,
        buffer_len(&document.buffer),
        &mut original,
    );
    let document_index = editor.current;
    editor.format_task = Some(idno_std::threads().spawn_owned(move || {
        profiling::scope!("format_document");
        let mut command = std::process::Command::new(formatter);
        command
            .args(arguments)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(_) => {
                return FormatResult {
                    document: document_index,
                    original,
                    formatted: Vec::new(),
                    formatter,
                    success: false,
                };
            }
        };
        let wrote_input = match child.stdin.take() {
            Some(mut input) => input.write_all(&original).is_ok(),
            None => false,
        };
        let output = child.wait_with_output();
        let (formatted, success) = match output {
            Ok(output) => (output.stdout, wrote_input && output.status.success()),
            Err(_) => (Vec::new(), false),
        };
        FormatResult {
            document: document_index,
            original,
            formatted,
            formatter,
            success,
        }
    }));
    write!(&mut editor.status, "formatting with {formatter}").unwrap();
}

fn editor_poll_format(editor: &mut Editor) -> bool {
    profiling::function_scope!();
    let complete = editor
        .format_task
        .as_ref()
        .is_some_and(idno_std::micropool::OwnedTask::complete);
    if !complete {
        return false;
    }
    let Some(task) = editor.format_task.take() else {
        return false;
    };
    let result = match task.try_join() {
        Ok(result) => result,
        Err(task) => {
            editor.format_task = Some(task);
            return false;
        }
    };
    if !result.success {
        write!(&mut editor.status, "{} failed", result.formatter).unwrap();
        return true;
    }
    let Some(document) = editor.documents.get_mut(result.document) else {
        return true;
    };
    let unchanged = result.original.len() == buffer_len(&document.buffer)
        && result
            .original
            .iter()
            .enumerate()
            .all(|(position, &byte)| buffer_byte(&document.buffer, position) == byte);
    if !unchanged {
        editor
            .status
            .push_str("format result discarded: buffer changed");
        return true;
    }
    if result.original == result.formatted {
        editor.status.push_str("already formatted");
        return true;
    }
    let cursor = document.cursor.min(result.formatted.len());
    let after = [SelectionState {
        cursor,
        anchor: cursor,
    }];
    let temp = idno_std::mem().scratch().temp();
    let mut replacements = temp.vec(1);
    replacements.push(Replacement {
        start: 0,
        end: buffer_len(&document.buffer),
        inserted: 0..result.formatted.len(),
    });
    document_replace_ranges(document, &mut replacements, &result.formatted, Some(&after));
    write!(&mut editor.status, "formatted with {}", result.formatter).unwrap();
    true
}

fn editor_set_document_path(editor: &mut Editor, path: &str) {
    let path = std::path::Path::new(path);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        editor.project.root.join(path)
    };
    let document = editor_document_mut(editor);
    document.path = Some(path);
    syntax_highlighting_set_path(&mut document.syntax, document.path.as_deref());
    code_index_set_path(&mut document.code_index, document.path.as_deref());
    git_gutter_set_path(&mut document.git_gutter, document.path.as_deref());
}

fn editor_open_path(editor: &mut Editor, path: &str) {
    profiling::function_scope!();
    let path = std::path::Path::new(path);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        editor.project.root.join(path)
    };
    if let Some(target) = editor_document_target(editor, path) {
        editor_switch_document(editor, target);
    }
}

fn editor_new_document(editor: &mut Editor) {
    document_commit_transaction(editor_document_mut(editor));
    editor.last_accessed_document = Some(editor.current);
    editor.documents.push(document_empty());
    editor.current = editor.documents.len() - 1;
    editor.mode = Mode::Normal;
    editor.jumps.clear();
    editor.jump_position = 0;
    editor.status.push_str("new scratch buffer");
}

fn editor_close_current_document(editor: &mut Editor, force: bool) {
    profiling::function_scope!();
    if editor_document(editor).modified && !force {
        editor
            .status
            .push_str("buffer has unsaved changes; use :bc!");
        return;
    }
    editor.documents.remove(editor.current);
    if editor.documents.is_empty() {
        editor.documents.push(document_empty());
        editor.current = 0;
    } else {
        editor.current = editor.current.min(editor.documents.len() - 1);
    }
    editor.last_accessed_document = None;
    editor.jumps.clear();
    editor.jump_position = 0;
    editor.mode = Mode::Normal;
}

fn editor_close_other_documents(editor: &mut Editor, force: bool) {
    profiling::function_scope!();
    if !force
        && editor
            .documents
            .iter()
            .enumerate()
            .any(|(index, document)| index != editor.current && document.modified)
    {
        editor
            .status
            .push_str("other buffers have unsaved changes; use :bco!");
        return;
    }
    let current = editor.documents.remove(editor.current);
    editor.documents.clear();
    editor.documents.push(current);
    editor.current = 0;
    editor.last_accessed_document = None;
    editor.jumps.clear();
    editor.jump_position = 0;
}

fn editor_close_all_documents(editor: &mut Editor, force: bool) {
    profiling::function_scope!();
    if !force && editor.documents.iter().any(|document| document.modified) {
        editor
            .status
            .push_str("buffers have unsaved changes; use :bca!");
        return;
    }
    editor.documents.clear();
    editor.documents.push(document_empty());
    editor.current = 0;
    editor.last_accessed_document = None;
    editor.jumps.clear();
    editor.jump_position = 0;
    editor.mode = Mode::Normal;
}

fn editor_set_theme_name(editor: &mut Editor, name: &str) {
    let Some(theme) = THEMES.iter().position(|theme| theme.name == name) else {
        write!(&mut editor.status, "unknown theme: {name}").unwrap();
        return;
    };
    editor_set_theme(editor, theme);
}

fn editor_set_theme(editor: &mut Editor, theme: usize) {
    let Some(theme_data) = THEMES.get(theme) else {
        return;
    };
    editor.theme = theme;
    write!(&mut editor.status, "theme {}", theme_data.name).unwrap();
}

fn editor_handle_insert_key(editor: &mut Editor, key: Key) -> bool {
    match key {
        Key::Escape => {
            editor.completion = None;
            editor_exit_insert(editor);
        }
        Key::Backspace => {
            let indentation_spaces = editor.config.indentation_spaces.max(1);
            let document = editor_document_mut(editor);
            let temp = idno_std::mem().scratch().temp();
            let mut replacements = temp.vec(document.insertion_points.len());
            for &position in &document.insertion_points {
                if position > 0 {
                    let previous = buffer_previous_char(&document.buffer, position);
                    let paired = position < buffer_len(&document.buffer)
                        && matches!(
                            (
                                buffer_byte(&document.buffer, previous),
                                buffer_byte(&document.buffer, position)
                            ),
                            (b'(', b')') | (b'[', b']') | (b'{', b'}') | (b'"', b'"')
                        );
                    let start = if paired {
                        previous
                    } else {
                        editor_indentation_backspace_start(
                            &document.buffer,
                            position,
                            indentation_spaces,
                        )
                    };
                    replacements.push(Replacement {
                        start,
                        end: if paired {
                            buffer_next_char(&document.buffer, position)
                        } else {
                            position
                        },
                        inserted: 0..0,
                    });
                }
            }
            document_replace_ranges(document, &mut replacements, &[], None);
            editor_refresh_completion(editor);
        }
        Key::Delete => {
            let document = editor_document_mut(editor);
            let temp = idno_std::mem().scratch().temp();
            let mut replacements = temp.vec(document.insertion_points.len());
            for &position in &document.insertion_points {
                let next = buffer_next_char(&document.buffer, position);
                if next > position {
                    replacements.push(Replacement {
                        start: position,
                        end: next,
                        inserted: 0..0,
                    });
                }
            }
            document_replace_ranges(document, &mut replacements, &[], None);
        }
        Key::Enter if editor.completion.is_some() => {
            editor_accept_completion(editor);
        }
        Key::Enter => {
            editor.completion = None;
            editor_insert_newline(editor);
        }
        Key::Tab => {
            if editor.completion.is_none() {
                editor_refresh_completion(editor);
            }
            if editor.completion.is_none()
                && editor_rust_completion_context(editor)
                && rust_method_index_finish(&mut editor.rust_methods)
            {
                editor_refresh_completion(editor);
            }
            if editor.completion.is_some() {
                editor_completion_tab(editor);
            } else if editor_rust_completion_context(editor) {
                editor.status.push_str("no completion candidates");
            } else {
                editor_insert_indentation(editor);
            }
        }
        Key::BackTab if editor.completion.is_some() => editor_select_completion(editor, false),
        Key::BackTab => {}
        Key::Character(character) => {
            let completion_previewed = editor
                .completion
                .as_ref()
                .is_some_and(|completion| completion.preview);
            let replace_placeholder = completion_previewed && editor_accept_completion(editor);
            let mut encoded = [0; 4];
            let encoded = character.encode_utf8(&mut encoded);
            if replace_placeholder && editor_replace_completion_placeholders(editor, encoded) {
                editor_refresh_completion(editor);
                return false;
            }
            if editor.config.flags.contains(EditorFlags::AUTO_PAIRS)
                && editor_insert_auto_pair(editor, character)
            {
                editor_refresh_completion(editor);
                return false;
            }
            editor_insert(editor, encoded);
            editor_refresh_completion(editor);
        }
        Key::Control(14) | Key::Down if editor.completion.is_some() => {
            editor_select_completion(editor, true)
        }
        Key::Control(16) | Key::Up if editor.completion.is_some() => {
            editor_select_completion(editor, false)
        }
        Key::Left => editor_move_insert_points_horizontal(editor, false),
        Key::Right => editor_move_insert_points_horizontal(editor, true),
        Key::Up => editor_move_insert_points_vertical(editor, false),
        Key::Down => editor_move_insert_points_vertical(editor, true),
        Key::Home => editor_move_insert_points_to_boundary(editor, false),
        Key::End => editor_move_insert_points_to_boundary(editor, true),
        Key::Control(_) | Key::Alt(_) => {}
    }
    false
}

fn editor_rust_completion_context(editor: &Editor) -> bool {
    if crate::syntax::syntax_language_from_path(editor_document(editor).path.as_deref())
        != crate::syntax::SyntaxLanguage::Rust
    {
        return false;
    }
    let document = editor_document(editor);
    let position = document
        .insertion_points
        .last()
        .copied()
        .unwrap_or(document.cursor);
    let mut start = position;
    while start > 0 && rust_identifier_byte(buffer_byte(&document.buffer, start - 1)) {
        start -= 1;
    }
    start > 0
        && (buffer_byte(&document.buffer, start - 1) == b'.'
            || (start >= 2
                && buffer_byte(&document.buffer, start - 1) == b':'
                && buffer_byte(&document.buffer, start - 2) == b':'))
}

fn editor_select_completion(editor: &mut Editor, forward: bool) {
    let Some(completion) = editor.completion.as_mut() else {
        return;
    };
    if completion.matches.is_empty() {
        return;
    }
    if !completion.preview {
        completion.preview = true;
        if !forward {
            completion.selected = completion.matches.len() - 1;
        }
        return;
    }
    completion.selected = if forward {
        (completion.selected + 1) % completion.matches.len()
    } else if completion.selected == 0 {
        completion.matches.len() - 1
    } else {
        completion.selected - 1
    };
}

fn editor_completion_tab(editor: &mut Editor) {
    let postfix = editor.completion.as_ref().is_some_and(|completion| {
        completion
            .matches
            .get(completion.selected)
            .is_some_and(|entry| entry.flags & COMPLETION_POSTFIX != 0)
    });
    if postfix {
        editor_accept_completion(editor);
    } else {
        editor_select_completion(editor, true);
    }
}

fn editor_indentation_backspace_start(
    buffer: &GapBuffer,
    position: usize,
    indentation_spaces: usize,
) -> usize {
    let previous = buffer_previous_char(buffer, position);
    if indentation_spaces <= 1 || buffer_byte(buffer, previous) != b' ' {
        return previous;
    }
    let line_start = buffer_line_start(buffer, position);
    let indentation_length = position - line_start;
    if indentation_length < indentation_spaces
        || !indentation_length.is_multiple_of(indentation_spaces)
    {
        return previous;
    }
    let unit_start = position - indentation_spaces;
    if (line_start..position).all(|index| matches!(buffer_byte(buffer, index), b' ' | b'\t'))
        && (unit_start..position).all(|index| buffer_byte(buffer, index) == b' ')
    {
        unit_start
    } else {
        previous
    }
}

fn editor_exit_insert(editor: &mut Editor) {
    profiling::function_scope!();
    editor.mode = Mode::Normal;
    let document = editor_document_mut(editor);
    let temp = idno_std::mem().scratch().temp();
    let mut replacements = temp.vec(document.insertion_points.len());
    for &insertion_point in &document.insertion_points {
        let line_start = buffer_line_start(&document.buffer, insertion_point);
        let line_end = buffer_line_end(&document.buffer, insertion_point);
        let mut only_indentation = line_start < line_end;
        let mut position = line_start;
        while position < line_end {
            if !matches!(buffer_byte(&document.buffer, position), b' ' | b'\t') {
                only_indentation = false;
                break;
            }
            position += 1;
        }
        if only_indentation {
            replacements.push(Replacement {
                start: line_start,
                end: line_end,
                inserted: 0..0,
            });
        }
    }
    document_replace_ranges(document, &mut replacements, &[], None);
    let edited = document
        .active_transaction
        .as_ref()
        .is_some_and(|transaction| !transaction.edits.is_empty());
    let mut selections = temp.vec(document.secondary_selections.len() + 1);
    document_selections(document, &mut selections);
    for (selection, &insertion_point) in selections.iter_mut().zip(&document.insertion_points) {
        let insertion_point = insertion_point.min(buffer_len(&document.buffer));
        let empty_line = buffer_line_start(&document.buffer, insertion_point)
            == buffer_line_end(&document.buffer, insertion_point);
        let head = if empty_line {
            insertion_point
        } else if edited && insertion_point > 0 {
            buffer_previous_char(&document.buffer, insertion_point)
        } else {
            command_cursor_clamped(&document.buffer, insertion_point, Mode::Normal)
        };
        let start = selection.anchor.min(selection.cursor);
        let end = selection.anchor.max(selection.cursor);
        if start == end {
            selection.anchor = head;
            selection.cursor = head;
            continue;
        }
        selection.anchor = if head.abs_diff(start) >= head.abs_diff(end) {
            start
        } else {
            end
        };
        selection.cursor = head;
    }
    document_set_selections(document, &selections);
    document.insertion_points.clear();
    document_commit_transaction(document);
}

fn editor_handle_command_key(editor: &mut Editor, key: Key) -> bool {
    match key {
        Key::Character(':') => editor_open_picker(editor, PickerKind::Commands),
        Key::Character('/') => editor_start_search(editor, SearchKind::Document),
        Key::Character('s') => editor_start_search(editor, SearchKind::Selection),
        Key::Character(' ') if editor.mode == Mode::Normal => {
            editor.pending_key = PendingKey::Space
        }
        Key::Character('m') => editor.pending_key = PendingKey::Match,
        Key::Character('g') => {
            editor.pending_count = 0;
            editor.pending_key = PendingKey::Goto;
        }
        Key::Character('[') => editor.pending_key = PendingKey::InsertLineAbove,
        Key::Character(']') => editor.pending_key = PendingKey::InsertLineBelow,
        Key::Character('i') if editor.mode == Mode::Normal => {
            editor_enter_insert(editor, false);
            editor.mode = Mode::Insert;
        }
        Key::Character('a') if editor.mode == Mode::Normal => {
            editor_enter_insert(editor, true);
            editor.mode = Mode::Insert;
        }
        Key::Character('A') if editor.mode == Mode::Normal => {
            editor_enter_insert_at_line_boundary(editor, true);
            editor.mode = Mode::Insert;
        }
        Key::Character('I') if editor.mode == Mode::Normal => {
            editor_enter_insert_at_line_boundary(editor, false);
            editor.mode = Mode::Insert;
        }
        Key::Character('o') if editor.mode == Mode::Normal => {
            document_begin_transaction(editor_document_mut(editor));
            editor_open_line_below(editor);
        }
        Key::Character('O') if editor.mode == Mode::Normal => {
            document_begin_transaction(editor_document_mut(editor));
            editor_open_line_above(editor);
        }
        Key::Character('C') if editor.mode == Mode::Normal || editor.mode == Mode::Select => {
            editor_add_cursor_below(editor)
        }
        Key::Character('v') => {
            editor.mode = if editor.mode == Mode::Select {
                Mode::Normal
            } else {
                Mode::Select
            };
        }
        Key::Escape => {
            editor.mode = Mode::Normal;
            let document = editor_document_mut(editor);
            let temp = idno_std::mem().scratch().temp();
            let mut selections = temp.vec(document.secondary_selections.len() + 1);
            document_selections(document, &mut selections);
            for selection in &mut selections {
                selection.anchor = selection.cursor;
            }
            document_set_selections(document, &selections);
        }
        Key::Character('h') | Key::Left => editor_move_horizontal(editor, false),
        Key::Character('l') | Key::Right => editor_move_horizontal(editor, true),
        Key::Character('k') | Key::Up => editor_move_vertical(editor, false),
        Key::Character('j') | Key::Down => editor_move_vertical(editor, true),
        Key::Tab => editor_jump(editor, true),
        Key::BackTab => editor_jump(editor, false),
        Key::Character('w') => editor_select_word_motion(editor, true),
        Key::Character('b') => editor_select_word_motion(editor, false),
        Key::Character('n') => editor_search_match(editor, true),
        Key::Character('N') => editor_search_match(editor, false),
        Key::Character('0') | Key::Home => editor_move_line_boundary(editor, false),
        Key::Character('$') | Key::End => editor_move_line_boundary(editor, true),
        Key::Character('x') => editor_select_lines(editor),
        Key::Character('X') => editor_select_current_lines(editor),
        Key::Character('d') | Key::Delete => editor_delete_selection(editor),
        Key::Character('y') => editor_yank(editor),
        Key::Character('R') => editor_replace_register(editor),
        Key::Character('p') => editor_paste_register(editor, true),
        Key::Character('P') => editor_paste_register(editor, false),
        Key::Character('c') => editor_change_selection(editor),
        Key::Character(';') => editor_collapse_selections(editor),
        Key::Character(',') => editor_keep_primary_selection(editor),
        Key::Alt(',') => editor_remove_primary_selection(editor),
        Key::Character('u') => document_undo(editor_document_mut(editor)),
        Key::Character('U') => document_redo(editor_document_mut(editor)),
        Key::Alt(_) => {}
        _ => {}
    }
    false
}

fn editor_request_quit(editor: &mut Editor) {
    let modified = editor.documents.iter().any(|document| document.modified);
    if modified && !editor.quit_warning {
        editor.quit_warning = true;
        editor
            .status
            .push_str("unsaved changes; quit again or Ctrl-Q to discard");
    } else {
        editor.quit_requested = true;
    }
}

fn editor_insert(editor: &mut Editor, text: &str) {
    let document = editor_document_mut(editor);
    let temp = idno_std::mem().scratch().temp();
    let mut replacements = temp.vec(document.insertion_points.len());
    for &position in &document.insertion_points {
        replacements.push(Replacement {
            start: position,
            end: position,
            inserted: 0..text.len(),
        });
    }
    document_replace_ranges(document, &mut replacements, text.as_bytes(), None);
    editor.quit_warning = false;
}

fn editor_replace_completion_placeholders(editor: &mut Editor, text: &str) -> bool {
    profiling::function_scope!();
    let document = editor_document_mut(editor);
    let temp = idno_std::mem().scratch().temp();
    let mut selections = temp.vec(document.secondary_selections.len() + 1);
    document_selections(document, &mut selections);
    if selections.is_empty()
        || selections
            .iter()
            .any(|selection| selection.anchor == selection.cursor)
    {
        return false;
    }
    selections.sort_unstable_by_key(|selection| selection.anchor.min(selection.cursor));
    let mut replacements = temp.vec(selections.len());
    for selection in &selections {
        replacements.push(Replacement {
            start: selection.anchor.min(selection.cursor),
            end: buffer_next_char(&document.buffer, selection.anchor.max(selection.cursor)),
            inserted: 0..text.len(),
        });
    }
    let mut after = temp.vec(replacements.len());
    let mut insertion_points = temp.vec(replacements.len());
    for replacement in &replacements {
        let start = offset_after_replacements(replacement.start, false, &replacements);
        let end = start + text.len();
        after.push(SelectionState {
            anchor: start,
            cursor: start,
        });
        insertion_points.push(end);
    }
    document_replace_ranges(document, &mut replacements, text.as_bytes(), Some(&after));
    document.insertion_points.clear();
    document
        .insertion_points
        .extend_from_slice(&insertion_points);
    editor.quit_warning = false;
    true
}

fn editor_insert_indentation(editor: &mut Editor) {
    let indentation_spaces = editor.config.indentation_spaces.max(1);
    let temp = idno_std::mem().scratch().temp();
    let mut indentation = temp.vec(indentation_spaces);
    indentation.extend(std::iter::repeat_n(b' ', indentation_spaces));
    let indentation = match std::str::from_utf8(&indentation) {
        Ok(indentation) => indentation,
        Err(_) => return,
    };
    editor_insert(editor, indentation);
}

fn editor_toggle_line_comments(editor: &mut Editor) {
    profiling::function_scope!();
    let token = match editor_document(editor).path.as_deref() {
        Some(path) => match path.extension().and_then(std::ffi::OsStr::to_str) {
            Some(
                "rs" | "c" | "h" | "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" | "go" | "jai"
                | "odin",
            ) => b"//".as_slice(),
            Some("toml" | "nim" | "nims" | "nimble" | "sh" | "bash" | "zsh" | "py") => {
                b"#".as_slice()
            }
            _ if path
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|name| {
                    matches!(name, ".bashrc" | ".bash_profile" | ".zshrc" | ".zprofile")
                }) =>
            {
                b"#".as_slice()
            }
            _ => {
                editor.status.push_str("no line comment for this language");
                return;
            }
        },
        None => {
            editor.status.push_str("no line comment for this buffer");
            return;
        }
    };
    let document = editor_document_mut(editor);
    let temp = idno_std::mem().scratch().temp();
    let mut selections = temp.vec(document.secondary_selections.len() + 1);
    document_selections(document, &mut selections);
    let mut lines = temp.vec(selections.len());
    for selection in &selections {
        let mut line = buffer_line_start(&document.buffer, selection.anchor.min(selection.cursor));
        let final_line =
            buffer_line_start(&document.buffer, selection.anchor.max(selection.cursor));
        loop {
            lines.push(line);
            if line >= final_line {
                break;
            }
            let end = buffer_line_end(&document.buffer, line);
            if end >= buffer_len(&document.buffer) {
                break;
            }
            line = end + 1;
        }
    }
    lines.sort_unstable();
    lines.dedup();
    let all_commented = !lines.is_empty()
        && lines.iter().all(|&line| {
            let indentation = buffer_line_indentation_end(&document.buffer, line);
            indentation + token.len() <= buffer_len(&document.buffer)
                && token.iter().enumerate().all(|(offset, &byte)| {
                    buffer_byte(&document.buffer, indentation + offset) == byte
                })
        });
    let inserted = if all_commented {
        b"".as_slice()
    } else if token == b"//" {
        b"// ".as_slice()
    } else {
        b"# ".as_slice()
    };
    let mut replacements = temp.vec(lines.len());
    for &line in &lines {
        let indentation = buffer_line_indentation_end(&document.buffer, line);
        let end = if all_commented {
            let after_token = indentation + token.len();
            after_token
                + usize::from(
                    after_token < buffer_len(&document.buffer)
                        && buffer_byte(&document.buffer, after_token) == b' ',
                )
        } else {
            indentation
        };
        replacements.push(Replacement {
            start: indentation,
            end,
            inserted: 0..inserted.len(),
        });
    }
    document_replace_ranges(document, &mut replacements, inserted, None);
}

fn buffer_line_indentation_end(buffer: &GapBuffer, line_start: usize) -> usize {
    let mut position = line_start;
    let line_end = buffer_line_end(buffer, line_start);
    while position < line_end && matches!(buffer_byte(buffer, position), b' ' | b'\t') {
        position += 1;
    }
    position
}

fn editor_insert_auto_pair(editor: &mut Editor, character: char) -> bool {
    profiling::function_scope!();
    if matches!(character, ')' | ']' | '}' | '"') {
        let document = editor_document_mut(editor);
        let all_already_closed = !document.insertion_points.is_empty()
            && document.insertion_points.iter().all(|&position| {
                position < buffer_len(&document.buffer)
                    && buffer_byte(&document.buffer, position) == character as u8
            });
        if all_already_closed {
            for position in &mut document.insertion_points {
                *position = buffer_next_char(&document.buffer, *position);
            }
            return true;
        }
    }
    let closing = match character {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        '"' => '"',
        _ => '\0',
    };
    if closing != '\0' {
        let pair = [character as u8, closing as u8];
        let document = editor_document_mut(editor);
        let temp = idno_std::mem().scratch().temp();
        let mut replacements = temp.vec(document.insertion_points.len());
        for &position in &document.insertion_points {
            replacements.push(Replacement {
                start: position,
                end: position,
                inserted: 0..2,
            });
        }
        document_replace_ranges(document, &mut replacements, &pair, None);
        for position in &mut document.insertion_points {
            *position = position.saturating_sub(1);
        }
        return true;
    }
    false
}

fn rust_callable_insertion(
    name: &str,
    detail: &str,
    receiver: Option<&str>,
    skip_self: bool,
    result: &mut Vec<u8, impl Allocator>,
) -> std::ops::Range<u32> {
    profiling::function_scope!();
    result.clear();
    result.extend_from_slice(name.as_bytes());
    let signature = detail.lines().next().unwrap_or("").as_bytes();
    let Some(mut position) = signature.iter().position(|&byte| byte == b'(') else {
        return 0..0;
    };
    position += 1;
    let mut close = position;
    let mut depth = 1usize;
    while close < signature.len() && depth > 0 {
        depth += usize::from(signature[close] == b'(');
        depth = depth.saturating_sub(usize::from(signature[close] == b')'));
        close += 1;
    }
    if depth != 0 {
        return 0..0;
    }
    result.push(b'(');
    let mut first_selection = 0..0;
    let mut parameter_start = position;
    let mut nested = 0usize;
    let mut emitted = 0usize;
    let mut parameter_index = 0usize;
    while position <= close - 1 {
        let at_end = position == close - 1;
        let byte = signature.get(position).copied().unwrap_or(b',');
        nested += usize::from(matches!(byte, b'(' | b'[' | b'{' | b'<'));
        nested = nested.saturating_sub(usize::from(matches!(byte, b')' | b']' | b'}' | b'>')));
        if (byte == b',' && nested == 0) || at_end {
            let mut start = parameter_start;
            let mut end = if at_end { close - 1 } else { position };
            while start < end && signature[start].is_ascii_whitespace() {
                start += 1;
            }
            while end > start && signature[end - 1].is_ascii_whitespace() {
                end -= 1;
            }
            if start < end {
                let colon = signature[start..end]
                    .iter()
                    .position(|&candidate| candidate == b':')
                    .map(|offset| start + offset);
                let self_parameter = colon.is_none()
                    && signature[start..end]
                        .windows(4)
                        .any(|candidate| candidate == b"self");
                if !(skip_self && self_parameter) {
                    if emitted > 0 {
                        result.extend_from_slice(b", ");
                    }
                    let argument_start = result.len() as u32;
                    if parameter_index == 0
                        && let Some(receiver) = receiver
                    {
                        if let Some(colon) = colon {
                            let mut type_start = colon + 1;
                            while type_start < end && signature[type_start].is_ascii_whitespace() {
                                type_start += 1;
                            }
                            if signature.get(type_start..type_start + 5) == Some(b"&mut ") {
                                result.extend_from_slice(b"&mut ");
                            } else if signature.get(type_start) == Some(&b'&') {
                                result.push(b'&');
                            }
                        }
                        result.extend_from_slice(receiver.as_bytes());
                    } else if let Some(colon) = colon {
                        let mut name_start = start;
                        while name_start < colon
                            && matches!(signature[name_start], b'&' | b' ' | b'\t')
                        {
                            name_start += 1;
                        }
                        if signature.get(name_start..name_start + 4) == Some(b"mut ") {
                            name_start += 4;
                        }
                        let mut name_end = colon;
                        while name_end > name_start && signature[name_end - 1].is_ascii_whitespace()
                        {
                            name_end -= 1;
                        }
                        let mut type_start = colon + 1;
                        while type_start < end && signature[type_start].is_ascii_whitespace() {
                            type_start += 1;
                        }
                        if signature.get(type_start..type_start + 5) == Some(b"&mut ") {
                            result.extend_from_slice(b"&mut ");
                        } else if signature.get(type_start) == Some(&b'&') {
                            result.push(b'&');
                        }
                        result.extend_from_slice(&signature[name_start..name_end]);
                    } else {
                        result.extend_from_slice(&signature[start..end]);
                    }
                    let argument_end = result.len() as u32;
                    if first_selection.is_empty() {
                        first_selection = argument_start..argument_end;
                    }
                    emitted += 1;
                }
                parameter_index += 1;
            }
            parameter_start = position + 1;
        }
        position += 1;
    }
    result.push(b')');
    first_selection
}

fn editor_refresh_completion(editor: &mut Editor) {
    profiling::function_scope!();
    if editor.mode != Mode::Insert
        || crate::syntax::syntax_language_from_path(editor_document(editor).path.as_deref())
            != crate::syntax::SyntaxLanguage::Rust
    {
        editor.completion = None;
        return;
    }
    let insertion_point = editor_document(editor)
        .insertion_points
        .last()
        .copied()
        .unwrap_or(editor_document(editor).cursor);
    let mut completion = editor.completion.take().unwrap_or(Completion {
        bytes: Vec::with_capacity(4096),
        matches: Vec::with_capacity(32),
        selected: 0,
        prefix_start: insertion_point,
        preview: false,
    });
    completion.bytes.clear();
    completion.matches.clear();
    completion.preview = false;
    let temp = idno_std::mem().scratch().temp();
    let mut insertion = temp.vec(128);
    if let Some((replacement_start, receiver_start, receiver_end)) =
        rust_postfix_dbg_expression(&editor_document(editor).buffer, insertion_point)
    {
        insertion.extend_from_slice(b"dbg!(");
        for position in receiver_start..receiver_end {
            insertion.push(buffer_byte(&editor_document(editor).buffer, position));
        }
        insertion.push(b')');
        completion_entry_push(
            &mut completion,
            CompletionCandidate {
                name: "dbg!",
                detail: "dbg!(value)\nPrints and returns the value without moving it.",
                insertion: &insertion,
                selection: 0..0,
                replacement_start,
                symbol: u32::MAX,
                flags: COMPLETION_POSTFIX,
            },
        );
        completion.prefix_start = replacement_start;
        completion.selected = 0;
        editor.completion = Some(completion);
        return;
    }
    let mut ranked = temp.vec(64);
    let method_prefix = rust_method_complete(
        &editor_document(editor).buffer,
        insertion_point,
        &editor.rust_methods.corpus,
        &mut ranked,
    );
    let mut free_ranked = temp.vec(64);
    let free_expression = rust_free_function_complete(
        &editor_document(editor).buffer,
        insertion_point,
        &editor.rust_methods.corpus,
        &mut free_ranked,
    );
    let prefix_start = match method_prefix.or(free_expression.map(|expression| expression.0)) {
        Some(prefix_start) => {
            for found in ranked.iter().take(32) {
                let name = rust_method_name(&editor.rust_methods.corpus, found.item);
                let detail = rust_method_detail(&editor.rust_methods.corpus, found.item);
                let selection = rust_callable_insertion(name, detail, None, true, &mut insertion);
                completion_entry_push(
                    &mut completion,
                    CompletionCandidate {
                        name,
                        detail,
                        insertion: &insertion,
                        selection,
                        replacement_start: prefix_start,
                        symbol: u32::MAX,
                        flags: 0,
                    },
                );
            }
            if let Some((_, receiver_start, receiver_end)) = free_expression {
                let mut receiver_bytes = temp.vec(receiver_end - receiver_start);
                for position in receiver_start..receiver_end {
                    receiver_bytes.push(buffer_byte(&editor_document(editor).buffer, position));
                }
                let receiver = std::str::from_utf8(&receiver_bytes).unwrap_or("");
                for found in free_ranked.iter().take(32) {
                    let symbol = found.item;
                    let name = rust_symbol_name(&editor.rust_methods.corpus, symbol);
                    let detail = rust_symbol_detail(&editor.rust_methods.corpus, symbol);
                    let selection = rust_callable_insertion(
                        name,
                        detail,
                        Some(receiver),
                        false,
                        &mut insertion,
                    );
                    completion_entry_push(
                        &mut completion,
                        CompletionCandidate {
                            name,
                            detail,
                            insertion: &insertion,
                            selection,
                            replacement_start: receiver_start,
                            symbol: symbol as u32,
                            flags: 0,
                        },
                    );
                }
            }
            prefix_start
        }
        None => {
            let mut prefix_start = insertion_point;
            while prefix_start > 0
                && rust_identifier_byte(buffer_byte(
                    &editor_document(editor).buffer,
                    prefix_start - 1,
                ))
            {
                prefix_start -= 1;
            }
            let reference_completion =
                rust_reference_completion(&editor_document(editor).buffer, insertion_point);
            let qualified_completion = insertion_point >= 2
                && buffer_byte(&editor_document(editor).buffer, insertion_point - 1) == b':'
                && buffer_byte(&editor_document(editor).buffer, insertion_point - 2) == b':';
            if prefix_start == insertion_point && !reference_completion && !qualified_completion {
                return;
            }
            editor_collect_name_completions(editor, prefix_start, insertion_point, &mut completion);
            prefix_start
        }
    };
    if completion.matches.is_empty() {
        return;
    }
    completion.prefix_start = prefix_start;
    completion.selected = completion
        .selected
        .min(completion.matches.len().saturating_sub(1));
    editor.completion = Some(completion);
}

fn rust_postfix_dbg_expression(
    buffer: &GapBuffer,
    insertion_point: usize,
) -> Option<(usize, usize, usize)> {
    const SUFFIX: &[u8] = b".dbg!";
    if insertion_point < SUFFIX.len()
        || !SUFFIX.iter().enumerate().all(|(offset, &byte)| {
            buffer_byte(buffer, insertion_point - SUFFIX.len() + offset) == byte
        })
    {
        return None;
    }
    let receiver_end = insertion_point - SUFFIX.len();
    let mut receiver_start = receiver_end;
    while receiver_start > 0 && rust_identifier_byte(buffer_byte(buffer, receiver_start - 1)) {
        receiver_start -= 1;
    }
    (receiver_start < receiver_end).then_some((receiver_start, receiver_start, receiver_end))
}

fn rust_reference_completion(buffer: &GapBuffer, insertion_point: usize) -> bool {
    if insertion_point > 0 && buffer_byte(buffer, insertion_point - 1) == b'&' {
        return true;
    }
    if insertion_point < 5 {
        return false;
    }
    let start = insertion_point - 5;
    (0..5).all(|offset| buffer_byte(buffer, start + offset) == b"&mut "[offset])
}

fn editor_accept_completion(editor: &mut Editor) -> bool {
    profiling::function_scope!();
    let Some(completion) = editor.completion.take() else {
        return false;
    };
    let Some(found) = completion.matches.get(completion.selected) else {
        return false;
    };
    let temp = idno_std::mem().scratch().temp();
    let insertion = completion_insertion(&completion, *found);
    let mut inserted = temp.vec(insertion.len());
    inserted.extend_from_slice(insertion);
    let primary_before_import = editor_document(editor)
        .insertion_points
        .last()
        .copied()
        .unwrap_or(editor_document(editor).cursor);
    let prefix_length = primary_before_import.saturating_sub(found.replacement_start);
    editor_insert_completion_import(editor, found.symbol);
    let document = editor_document_mut(editor);
    let mut replacements = temp.vec(document.insertion_points.len());
    for &position in &document.insertion_points {
        replacements.push(Replacement {
            start: position.saturating_sub(prefix_length),
            end: position,
            inserted: 0..inserted.len(),
        });
    }
    replacements.sort_unstable_by_key(|replacement| replacement.start);
    let mut after = temp.vec(replacements.len());
    let mut insertion_points = temp.vec(replacements.len());
    let mut shift = 0isize;
    for replacement in &replacements {
        let after_start = (replacement.start as isize + shift) as usize;
        let selection_start = after_start + found.selection_start as usize;
        let selection_end = after_start + found.selection_end as usize;
        let cursor = if selection_end > selection_start {
            selection_end - 1
        } else {
            after_start + inserted.len()
        };
        after.push(SelectionState {
            anchor: selection_start,
            cursor,
        });
        insertion_points.push(if selection_end > selection_start {
            selection_start
        } else {
            after_start + inserted.len()
        });
        shift += inserted.len() as isize - (replacement.end - replacement.start) as isize;
    }
    document_replace_ranges(document, &mut replacements, &inserted, Some(&after));
    document.insertion_points.clear();
    document
        .insertion_points
        .extend_from_slice(&insertion_points);
    found.selection_end > found.selection_start
}

fn editor_insert_completion_import(editor: &mut Editor, symbol: u32) {
    profiling::function_scope!();
    if symbol == u32::MAX {
        return;
    }
    let symbol = symbol as usize;
    let Some(path) = rust_symbol_path(&editor.rust_methods.corpus, symbol) else {
        return;
    };
    if editor_document(editor).path.as_deref() == Some(path) {
        return;
    }
    let temp = idno_std::mem().scratch().temp();
    let mut module = temp.vec(64);
    let relative = path.strip_prefix(&editor.project.root).unwrap_or(path);
    let file_name = relative.file_name();
    let mut after_source = false;
    for component in relative.components() {
        let component_name = component.as_os_str();
        let bytes = component_name.as_encoded_bytes();
        if bytes == b"src" {
            module.clear();
            module.extend_from_slice(b"crate");
            after_source = true;
            continue;
        }
        if !after_source {
            continue;
        }
        let file = file_name == Some(component_name);
        let mut component_bytes = bytes;
        if file && let Some(dot) = component_bytes.iter().rposition(|&byte| byte == b'.') {
            component_bytes = &component_bytes[..dot];
        }
        if file && matches!(component_bytes, b"lib" | b"main" | b"mod") {
            continue;
        }
        module.extend_from_slice(b"::");
        module.extend_from_slice(component_bytes);
    }
    if !after_source || module.is_empty() {
        return;
    }
    let symbol_name = rust_symbol_name(&editor.rust_methods.corpus, symbol).as_bytes();
    let mut name = temp.vec(symbol_name.len());
    name.extend_from_slice(symbol_name);
    let document = editor_document_mut(editor);
    let mut line_start = 0usize;
    let mut insertion_point = 0usize;
    while line_start < buffer_len(&document.buffer) {
        let line_end = buffer_line_end(&document.buffer, line_start);
        let mut text = line_start;
        while text < line_end && buffer_byte(&document.buffer, text).is_ascii_whitespace() {
            text += 1;
        }
        let use_line = buffer_range_matches_identifier(&document.buffer, text, line_end, b"use");
        let top_level_prefix = use_line
            || buffer_range_matches_identifier(&document.buffer, text, line_end, b"mod")
            || text == line_end
            || (text + 1 < line_end
                && buffer_byte(&document.buffer, text) == b'#'
                && buffer_byte(&document.buffer, text + 1) == b'!')
            || (text + 2 < line_end
                && buffer_byte(&document.buffer, text) == b'/'
                && buffer_byte(&document.buffer, text + 1) == b'/'
                && buffer_byte(&document.buffer, text + 2) == b'!');
        if !top_level_prefix {
            break;
        }
        if use_line && buffer_line_contains_identifier(&document.buffer, text + 3, line_end, &name)
        {
            return;
        }
        insertion_point = if line_end < buffer_len(&document.buffer) {
            line_end + 1
        } else {
            line_end
        };
        line_start = line_end.saturating_add(1);
    }
    let mut import = temp.vec(module.len() + name.len() + 9);
    import.extend_from_slice(b"use ");
    import.extend_from_slice(&module);
    import.extend_from_slice(b"::");
    import.extend_from_slice(&name);
    import.extend_from_slice(b";\n");
    let mut replacements = temp.vec(1);
    replacements.push(Replacement {
        start: insertion_point,
        end: insertion_point,
        inserted: 0..import.len(),
    });
    document_replace_ranges(document, &mut replacements, &import, None);
}

fn buffer_line_contains_identifier(
    buffer: &GapBuffer,
    start: usize,
    end: usize,
    name: &[u8],
) -> bool {
    if name.is_empty() || end.saturating_sub(start) < name.len() {
        return false;
    }
    (start..=end - name.len())
        .any(|position| buffer_range_matches_identifier(buffer, position, end, name))
}

fn editor_collect_name_completions(
    editor: &mut Editor,
    prefix_start: usize,
    insertion_point: usize,
    completion: &mut Completion,
) {
    profiling::function_scope!();
    {
        let document = editor_document_mut(editor);
        while !document.code_index.complete {
            code_index_step(
                &document.buffer,
                &mut document.code_index,
                256 * 1024,
                std::time::Duration::MAX,
            );
        }
    }
    let temp = idno_std::mem().scratch().temp();
    let mut query_bytes = temp.vec(insertion_point - prefix_start);
    for position in prefix_start..insertion_point {
        query_bytes.push(buffer_byte(&editor_document(editor).buffer, position));
    }
    let query = match std::str::from_utf8(&query_bytes) {
        Ok(query) => query,
        Err(_) => return,
    };
    let mut qualifier = temp.vec(32);
    let mut parent_qualifier = temp.vec(32);
    if prefix_start >= 2
        && buffer_byte(&editor_document(editor).buffer, prefix_start - 1) == b':'
        && buffer_byte(&editor_document(editor).buffer, prefix_start - 2) == b':'
    {
        let mut start = prefix_start - 2;
        while start > 0
            && rust_identifier_byte(buffer_byte(&editor_document(editor).buffer, start - 1))
        {
            start -= 1;
        }
        for position in start..prefix_start - 2 {
            qualifier.push(buffer_byte(&editor_document(editor).buffer, position));
        }
        if start >= 2
            && buffer_byte(&editor_document(editor).buffer, start - 1) == b':'
            && buffer_byte(&editor_document(editor).buffer, start - 2) == b':'
        {
            let parent_end = start - 2;
            let mut parent_start = parent_end;
            while parent_start > 0
                && rust_identifier_byte(buffer_byte(
                    &editor_document(editor).buffer,
                    parent_start - 1,
                ))
            {
                parent_start -= 1;
            }
            for position in parent_start..parent_end {
                parent_qualifier.push(buffer_byte(&editor_document(editor).buffer, position));
            }
        }
    }

    let document = editor_document(editor);
    let mut expected_type = temp.vec(32);
    let expected_borrow = rust_call_argument_type(
        &document.buffer,
        insertion_point,
        &editor.rust_methods.corpus,
        &mut expected_type,
    );
    let mut local_bytes = temp.vec(1024);
    let mut local_ranges = temp.vec(document.code_index.symbols.len());
    let mut local_identifiers = temp.vec(document.code_index.symbols.len());
    for symbol in &document.code_index.symbols {
        if !qualifier.is_empty() {
            break;
        }
        let Some(identifier) = document
            .code_index
            .identifiers
            .get(symbol.identifier as usize)
        else {
            continue;
        };
        if identifier.start as usize >= insertion_point {
            continue;
        }
        let start = local_bytes.len();
        for position in identifier.start as usize..identifier.end as usize {
            local_bytes.push(buffer_byte(&document.buffer, position));
        }
        local_ranges.push(start..local_bytes.len());
        local_identifiers.push(*identifier);
    }
    let mut labels = temp.vec(local_ranges.len());
    for range in &local_ranges {
        labels.push(std::str::from_utf8(&local_bytes[range.clone()]).unwrap_or(""));
    }
    let mut local_matches = temp.vec(local_ranges.len());
    fuzzy_rank(query, &labels, &mut local_matches);
    let mut local_type = temp.vec(32);
    let mut local_insertion = temp.vec(64);
    for compatible_only in [true, false] {
        for found in local_matches.iter().take(32) {
            let identifier = local_identifiers[found.item];
            let compatible = expected_borrow.is_some()
                && rust_binding_type_at(
                    &document.buffer,
                    identifier.start as usize,
                    identifier.end as usize,
                    &editor.rust_methods.corpus,
                    &mut local_type,
                )
                && local_type == expected_type;
            if compatible != compatible_only {
                continue;
            }
            let name = labels[found.item];
            local_insertion.clear();
            if compatible {
                match expected_borrow {
                    Some(RustBorrowKind::Shared) => local_insertion.push(b'&'),
                    Some(RustBorrowKind::Mutable) => local_insertion.extend_from_slice(b"&mut "),
                    Some(RustBorrowKind::Owned) | None => {}
                }
            }
            local_insertion.extend_from_slice(name.as_bytes());
            completion_entry_push(
                completion,
                CompletionCandidate {
                    name,
                    detail: "local",
                    insertion: &local_insertion,
                    selection: 0..0,
                    replacement_start: prefix_start,
                    symbol: u32::MAX,
                    flags: 0,
                },
            );
        }
    }

    for &keyword in RUST_COMPLETION_KEYWORDS {
        if !qualifier.is_empty() {
            break;
        }
        if slice_starts_with_smart_case(keyword.as_bytes(), query.as_bytes()) {
            completion_entry_push(
                completion,
                CompletionCandidate {
                    name: keyword,
                    detail: "Rust keyword",
                    insertion: keyword.as_bytes(),
                    selection: 0..0,
                    replacement_start: prefix_start,
                    symbol: u32::MAX,
                    flags: 0,
                },
            );
        }
    }

    let mut symbol_labels = temp.vec(256);
    let mut symbol_indices = temp.vec(256);
    let namespace_root = if qualifier.is_empty() {
        None
    } else {
        rust_namespace_root(&editor.rust_methods.corpus, &qualifier)
    };
    let qualified_owner_path = (!parent_qualifier.is_empty())
        .then(|| {
            (0..editor.rust_methods.corpus.symbols.len()).find_map(|symbol| {
                (rust_symbol_name(&editor.rust_methods.corpus, symbol).as_bytes() == qualifier
                    && rust_symbol_path(&editor.rust_methods.corpus, symbol)
                        .is_some_and(|path| rust_path_module_matches(path, &parent_qualifier)))
                .then(|| rust_symbol_path(&editor.rust_methods.corpus, symbol))
                .flatten()
            })
        })
        .flatten();
    for symbol in 0..editor.rust_methods.corpus.symbols.len() {
        let label = rust_symbol_name(&editor.rust_methods.corpus, symbol);
        let qualified_match = if qualifier.is_empty() {
            true
        } else if let Some(owner_path) = qualified_owner_path {
            rust_symbol_owner(&editor.rust_methods.corpus, symbol).as_bytes() == qualifier
                && rust_symbol_path(&editor.rust_methods.corpus, symbol) == Some(owner_path)
        } else {
            rust_symbol_owner(&editor.rust_methods.corpus, symbol).as_bytes() == qualifier
                || rust_symbol_path(&editor.rust_methods.corpus, symbol).is_some_and(|path| {
                    rust_path_module_matches(path, &qualifier)
                        || namespace_root.is_some_and(|root| path.starts_with(root))
                })
        };
        if qualified_match && slice_starts_with_smart_case(label.as_bytes(), query.as_bytes()) {
            symbol_labels.push(label);
            symbol_indices.push(symbol);
        }
    }
    let mut symbol_matches = temp.vec(symbol_labels.len());
    fuzzy_rank(query, &symbol_labels, &mut symbol_matches);
    let mut insertion = temp.vec(128);
    for found in symbol_matches.iter().take(64) {
        let symbol = symbol_indices[found.item];
        let label = symbol_labels[found.item];
        let detail = rust_symbol_detail(&editor.rust_methods.corpus, symbol);
        let detail = if detail.is_empty() {
            rust_symbol_path(&editor.rust_methods.corpus, symbol)
                .and_then(std::path::Path::to_str)
                .unwrap_or("symbol")
        } else {
            detail
        };
        let selection = rust_callable_insertion(label, detail, None, false, &mut insertion);
        completion_entry_push(
            completion,
            CompletionCandidate {
                name: label,
                detail,
                insertion: &insertion,
                selection,
                replacement_start: prefix_start,
                symbol: if qualifier.is_empty() {
                    symbol as u32
                } else {
                    u32::MAX
                },
                flags: 0,
            },
        );
        if completion.matches.len() >= 32 {
            break;
        }
    }
}

fn slice_starts_with_smart_case(candidate: &[u8], prefix: &[u8]) -> bool {
    candidate.len() >= prefix.len()
        && prefix
            .iter()
            .zip(candidate)
            .all(|(&query, &candidate)| fuzzy_byte_matches(query, candidate))
}

fn completion_entry_push(completion: &mut Completion, candidate: CompletionCandidate<'_>) {
    if candidate.name.is_empty()
        || completion.matches.iter().any(|entry| {
            &completion.bytes[entry.name_start as usize..entry.name_end as usize]
                == candidate.name.as_bytes()
        })
        || completion.bytes.len()
            + candidate.name.len()
            + candidate.detail.len()
            + candidate.insertion.len()
            > u32::MAX as usize
    {
        return;
    }
    let name_start = completion.bytes.len() as u32;
    completion
        .bytes
        .extend_from_slice(candidate.name.as_bytes());
    let name_end = completion.bytes.len() as u32;
    let detail_start = name_end;
    completion
        .bytes
        .extend_from_slice(candidate.detail.as_bytes());
    let detail_end = completion.bytes.len() as u32;
    let insertion_start = detail_end;
    completion.bytes.extend_from_slice(candidate.insertion);
    let insertion_end = completion.bytes.len() as u32;
    completion.matches.push(CompletionEntry {
        name_start,
        name_end,
        detail_start,
        detail_end,
        insertion_start,
        insertion_end,
        selection_start: candidate.selection.start,
        selection_end: candidate.selection.end,
        replacement_start: candidate.replacement_start,
        symbol: candidate.symbol,
        flags: candidate.flags,
    });
}

fn completion_name(completion: &Completion, entry: CompletionEntry) -> &str {
    std::str::from_utf8(&completion.bytes[entry.name_start as usize..entry.name_end as usize])
        .unwrap_or("")
}

fn completion_detail(completion: &Completion, entry: CompletionEntry) -> &str {
    std::str::from_utf8(&completion.bytes[entry.detail_start as usize..entry.detail_end as usize])
        .unwrap_or("")
}

fn completion_insertion(completion: &Completion, entry: CompletionEntry) -> &[u8] {
    &completion.bytes[entry.insertion_start as usize..entry.insertion_end as usize]
}

fn editor_enter_insert(editor: &mut Editor, after: bool) {
    let document = editor_document_mut(editor);
    document_begin_transaction(document);
    let temp = idno_std::mem().scratch().temp();
    let mut selections = temp.vec(document.secondary_selections.len() + 1);
    document_selections(document, &mut selections);
    document.insertion_points.clear();
    document.insertion_points.reserve(selections.len());
    document
        .insertion_points
        .extend(selections.iter().map(|selection| {
            let start = selection.anchor.min(selection.cursor);
            let end = selection.anchor.max(selection.cursor);
            if after {
                buffer_next_char(&document.buffer, end)
            } else {
                start
            }
        }));
}

fn editor_enter_insert_at_line_boundary(editor: &mut Editor, end: bool) {
    let document = editor_document_mut(editor);
    document_begin_transaction(document);
    let temp = idno_std::mem().scratch().temp();
    let mut selections = temp.vec(document.secondary_selections.len() + 1);
    document_selections(document, &mut selections);
    document.insertion_points.clear();
    document.insertion_points.reserve(selections.len());
    document
        .insertion_points
        .extend(selections.iter().map(|selection| {
            if end {
                buffer_line_end(&document.buffer, selection.cursor)
            } else {
                let mut position = buffer_line_start(&document.buffer, selection.cursor);
                let line_end = buffer_line_end(&document.buffer, position);
                while position < line_end
                    && matches!(buffer_byte(&document.buffer, position), b' ' | b'\t')
                {
                    position += 1;
                }
                position
            }
        }));
}

fn editor_delete_selection(editor: &mut Editor) {
    let mode = editor.mode;
    let document = editor_document_mut(editor);
    let length = buffer_len(&document.buffer);
    if length == 0 {
        return;
    }
    let temp = idno_std::mem().scratch().temp();
    let mut selections = temp.vec(document.secondary_selections.len() + 1);
    document_selections(document, &mut selections);
    let mut replacements = temp.vec(selections.len());
    for &selection in &selections {
        let (start, end) = if mode != Mode::Insert {
            let start = selection.anchor.min(selection.cursor);
            let head = selection.anchor.max(selection.cursor).min(length);
            (start, buffer_next_char(&document.buffer, head).min(length))
        } else {
            let start = selection.cursor.min(length);
            (start, buffer_next_char(&document.buffer, start))
        };
        if start < end {
            replacements.push(Replacement {
                start,
                end,
                inserted: 0..0,
            });
        }
    }
    if !replacements.is_empty() {
        document_replace_ranges(document, &mut replacements, &[], None);
        editor.mode = Mode::Normal;
        editor.quit_warning = false;
    }
}

fn editor_change_selection(editor: &mut Editor) {
    document_begin_transaction(editor_document_mut(editor));
    editor_delete_selection(editor);
    let document = editor_document_mut(editor);
    let temp = idno_std::mem().scratch().temp();
    let mut selections = temp.vec(document.secondary_selections.len() + 1);
    document_selections(document, &mut selections);
    document.insertion_points.clear();
    document.insertion_points.reserve(selections.len());
    document
        .insertion_points
        .extend(selections.iter().map(|selection| selection.cursor));
    editor.mode = Mode::Insert;
}

fn editor_collapse_selections(editor: &mut Editor) {
    let document = editor_document_mut(editor);
    let temp = idno_std::mem().scratch().temp();
    let mut selections = temp.vec(document.secondary_selections.len() + 1);
    document_selections(document, &mut selections);
    for selection in &mut selections {
        selection.anchor = selection.cursor;
    }
    document_set_selections(document, &selections);
}

fn editor_keep_primary_selection(editor: &mut Editor) {
    editor_document_mut(editor).secondary_selections.clear();
}

fn editor_remove_primary_selection(editor: &mut Editor) {
    let document = editor_document_mut(editor);
    let Some(primary) = document.secondary_selections.pop() else {
        return;
    };
    document.cursor = primary.cursor;
    document.anchor = primary.anchor;
}

fn editor_yank(editor: &mut Editor) {
    profiling::function_scope!();
    let document = editor_document(editor);
    let temp = idno_std::mem().scratch().temp();
    let mut selections = temp.vec(document.secondary_selections.len() + 1);
    document_selections(document, &mut selections);
    let mut bytes = temp.vec(1024);
    let mut values = temp.vec(selections.len());
    for selection in selections {
        let start = selection.anchor.min(selection.cursor);
        let end = buffer_next_char(&document.buffer, selection.anchor.max(selection.cursor));
        let value_start = bytes.len();
        buffer_append_range(&document.buffer, start, end, &mut bytes);
        let value_end = bytes.len();
        if value_end <= u32::MAX as usize {
            values.push(value_start as u32..value_end as u32);
        }
    }
    editor.register.bytes.clear();
    editor.register.bytes.extend_from_slice(&bytes);
    editor.register.values.clear();
    editor.register.values.extend(values);
    write!(
        &mut editor.status,
        "yanked {} selection(s)",
        editor.register.values.len()
    )
    .unwrap();
}

fn editor_paste_register(editor: &mut Editor, after: bool) {
    profiling::function_scope!();
    if editor.register.values.is_empty() {
        editor.status.push_str("register is empty");
        return;
    }
    let temp = idno_std::mem().scratch().temp();
    let mut bytes = temp.vec(editor.register.bytes.len());
    bytes.extend_from_slice(&editor.register.bytes);
    let mut values = temp.vec(editor.register.values.len());
    values.extend(editor.register.values.iter().cloned());
    editor_paste_values(editor, &bytes, &values, after);
}

fn editor_replace_register(editor: &mut Editor) {
    profiling::function_scope!();
    if editor.register.values.is_empty() {
        editor.status.push_str("register is empty");
        return;
    }
    let temp = idno_std::mem().scratch().temp();
    let mut bytes = temp.vec(editor.register.bytes.len());
    bytes.extend_from_slice(&editor.register.bytes);
    let mut values = temp.vec(editor.register.values.len());
    values.extend(editor.register.values.iter().cloned());
    editor_replace_values(editor, &bytes, &values);
}

fn editor_paste_values(
    editor: &mut Editor,
    bytes: &[u8],
    values: &[std::ops::Range<u32>],
    after: bool,
) {
    profiling::function_scope!();
    if bytes.is_empty() || values.is_empty() {
        return;
    }
    if values
        .iter()
        .any(|value| value.start > value.end || value.end as usize > bytes.len())
    {
        editor.status.push_str("invalid paste register");
        return;
    }
    let document = editor_document_mut(editor);
    let temp = idno_std::mem().scratch().temp();
    let mut selections = temp.vec(document.secondary_selections.len() + 1);
    document_selections(document, &mut selections);
    selections.sort_unstable_by_key(|selection| selection.anchor.min(selection.cursor));
    let mut replacements = temp.vec(selections.len());
    for (selection_index, selection) in selections.iter().enumerate() {
        let position = if after {
            buffer_next_char(&document.buffer, selection.anchor.max(selection.cursor))
        } else {
            selection.anchor.min(selection.cursor)
        };
        let value = values[selection_index % values.len()].clone();
        replacements.push(Replacement {
            start: position,
            end: position,
            inserted: value.start as usize..value.end as usize,
        });
    }
    replacements.sort_unstable_by_key(|replacement| replacement.start);
    let mut pasted = temp.vec(replacements.len());
    for replacement in &replacements {
        let start = offset_after_replacements(replacement.start, false, &replacements);
        let end = start + replacement.inserted.len();
        let mut last_character = replacement.inserted.end.saturating_sub(1);
        while last_character > replacement.inserted.start
            && bytes[last_character] & 0b1100_0000 == 0b1000_0000
        {
            last_character -= 1;
        }
        pasted.push(SelectionState {
            anchor: start,
            cursor: if end > start {
                start + last_character - replacement.inserted.start
            } else {
                start
            },
        });
    }
    document_replace_ranges(document, &mut replacements, bytes, Some(&pasted));
}

fn editor_yank_system(editor: &mut Editor) {
    profiling::function_scope!();
    editor_yank(editor);
    if let Some(task) = editor.clipboard_copy_task.take() {
        task.cancel();
    }
    let bytes = editor.register.bytes.clone();
    editor.clipboard_copy_task =
        Some(idno_std::threads().spawn_owned(move || clipboard_write(&bytes)));
    editor.status.clear();
    editor.status.push_str("copying to system clipboard");
}

fn editor_paste_system(editor: &mut Editor) {
    profiling::function_scope!();
    if let Some(task) = editor.clipboard_paste_task.take() {
        task.cancel();
    }
    editor.clipboard_paste_replaces = false;
    editor.clipboard_paste_task = Some(idno_std::threads().spawn_owned(clipboard_read));
    editor.status.push_str("reading system clipboard");
}

fn editor_replace_system(editor: &mut Editor) {
    profiling::function_scope!();
    if let Some(task) = editor.clipboard_paste_task.take() {
        task.cancel();
    }
    editor.clipboard_paste_replaces = true;
    editor.clipboard_paste_task = Some(idno_std::threads().spawn_owned(clipboard_read));
    editor.status.push_str("reading system clipboard");
}

fn editor_replace_selections(editor: &mut Editor, bytes: &[u8]) {
    profiling::function_scope!();
    let value = 0..bytes.len() as u32;
    editor_replace_values(editor, bytes, std::slice::from_ref(&value));
}

fn editor_replace_values(editor: &mut Editor, bytes: &[u8], values: &[std::ops::Range<u32>]) {
    profiling::function_scope!();
    if values.is_empty()
        || values
            .iter()
            .any(|value| value.start > value.end || value.end as usize > bytes.len())
    {
        editor.status.push_str("invalid replace register");
        return;
    }
    let document = editor_document_mut(editor);
    let temp = idno_std::mem().scratch().temp();
    let mut selections = temp.vec(document.secondary_selections.len() + 1);
    document_selections(document, &mut selections);
    selections.sort_unstable_by_key(|selection| selection.anchor.min(selection.cursor));
    let mut replacements = temp.vec(selections.len());
    for (selection_index, selection) in selections.iter().enumerate() {
        let value = values[selection_index % values.len()].clone();
        replacements.push(Replacement {
            start: selection.anchor.min(selection.cursor),
            end: buffer_next_char(&document.buffer, selection.anchor.max(selection.cursor)),
            inserted: value.start as usize..value.end as usize,
        });
    }
    replacements.sort_unstable_by_key(|replacement| replacement.start);
    let mut replaced = temp.vec(replacements.len());
    for replacement in &replacements {
        let start = offset_after_replacements(replacement.start, false, &replacements);
        let end = start + replacement.inserted.len();
        let mut last_character = replacement.inserted.end.saturating_sub(1);
        while last_character > replacement.inserted.start
            && bytes[last_character] & 0b1100_0000 == 0b1000_0000
        {
            last_character -= 1;
        }
        replaced.push(SelectionState {
            anchor: start,
            cursor: if end > start {
                start + last_character - replacement.inserted.start
            } else {
                start
            },
        });
    }
    document_replace_ranges(document, &mut replacements, bytes, Some(&replaced));
}

fn editor_poll_clipboard(editor: &mut Editor) -> bool {
    profiling::function_scope!();
    let copy_complete = editor
        .clipboard_copy_task
        .as_ref()
        .is_some_and(idno_std::micropool::OwnedTask::complete);
    if copy_complete && let Some(task) = editor.clipboard_copy_task.take() {
        match task.try_join() {
            Ok(true) => editor.status.push_str("copied to system clipboard"),
            Ok(false) => editor.status.push_str("system clipboard unavailable"),
            Err(task) => editor.clipboard_copy_task = Some(task),
        }
        return true;
    }
    let paste_complete = editor
        .clipboard_paste_task
        .as_ref()
        .is_some_and(idno_std::micropool::OwnedTask::complete);
    if !paste_complete {
        return false;
    }
    let Some(task) = editor.clipboard_paste_task.take() else {
        return false;
    };
    let paste = match task.try_join() {
        Ok(paste) => paste,
        Err(task) => {
            editor.clipboard_paste_task = Some(task);
            return false;
        }
    };
    if !paste.available {
        editor.status.push_str("system clipboard unavailable");
        return true;
    }
    if paste.bytes.len() > u32::MAX as usize {
        editor.status.push_str("system clipboard is too large");
        return true;
    }
    let range = 0..paste.bytes.len() as u32;
    if editor.clipboard_paste_replaces {
        editor_replace_selections(editor, &paste.bytes);
    } else {
        editor_paste_values(editor, &paste.bytes, std::slice::from_ref(&range), true);
    }
    true
}

fn clipboard_write(bytes: &[u8]) -> bool {
    profiling::function_scope!();
    let commands: [(&str, &[&str]); 3] = [
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ];
    for (program, arguments) in commands {
        let child = std::process::Command::new(program)
            .args(arguments)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        let Ok(mut child) = child else {
            continue;
        };
        let written = child
            .stdin
            .take()
            .is_some_and(|mut input| input.write_all(bytes).is_ok());
        let succeeded = child.wait().is_ok_and(|status| status.success());
        if written && succeeded {
            return true;
        }
    }
    false
}

fn clipboard_read() -> ClipboardPaste {
    profiling::function_scope!();
    let commands: [(&str, &[&str]); 3] = [
        ("wl-paste", &["--no-newline"]),
        ("xclip", &["-selection", "clipboard", "-out"]),
        ("xsel", &["--clipboard", "--output"]),
    ];
    for (program, arguments) in commands {
        let output = std::process::Command::new(program)
            .args(arguments)
            .stderr(std::process::Stdio::null())
            .output();
        if let Ok(output) = output
            && output.status.success()
        {
            return ClipboardPaste {
                bytes: output.stdout,
                available: true,
            };
        }
    }
    ClipboardPaste {
        bytes: Vec::new(),
        available: false,
    }
}

fn editor_select_lines(editor: &mut Editor) {
    let document = editor_document_mut(editor);
    let length = buffer_len(&document.buffer);
    let temp = idno_std::mem().scratch().temp();
    let mut selections = temp.vec(document.secondary_selections.len() + 1);
    document_selections(document, &mut selections);
    for selection in &mut selections {
        let selection_start = selection.anchor.min(selection.cursor);
        let selection_end = selection.anchor.max(selection.cursor);
        let line_start = buffer_line_start(&document.buffer, selection_start);
        let line_end = buffer_line_end(&document.buffer, selection_end);
        let already_linewise = selection.anchor == line_start && selection.cursor == line_end;
        selection.anchor = line_start;
        if already_linewise && line_end < length && buffer_byte(&document.buffer, line_end) == b'\n'
        {
            selection.cursor = buffer_line_end(&document.buffer, line_end + 1);
        } else {
            selection.cursor = line_end;
        }
        if selection.cursor == length
            && selection.cursor > line_start
            && buffer_byte(&document.buffer, selection.cursor - 1) != b'\n'
        {
            selection.cursor = buffer_previous_char(&document.buffer, selection.cursor);
        }
    }
    document_set_selections(document, &selections);
}

fn editor_select_current_lines(editor: &mut Editor) {
    let document = editor_document_mut(editor);
    let temp = idno_std::mem().scratch().temp();
    let mut selections = temp.vec(document.secondary_selections.len() + 1);
    document_selections(document, &mut selections);
    for selection in &mut selections {
        let start = buffer_line_start(&document.buffer, selection.cursor);
        let end = buffer_line_end(&document.buffer, selection.cursor);
        if start == end {
            continue;
        }
        selection.anchor = start;
        selection.cursor = buffer_previous_char(&document.buffer, end);
    }
    document_set_selections(document, &selections);
}

fn editor_select_word_motion(editor: &mut Editor, forward: bool) {
    let normal = editor.mode == Mode::Normal;
    let document = editor_document_mut(editor);
    let temp = idno_std::mem().scratch().temp();
    let mut selections = temp.vec(document.secondary_selections.len() + 1);
    document_selections(document, &mut selections);
    for selection in &mut selections {
        if forward {
            let start = if normal && selection.anchor != selection.cursor {
                buffer_next_char(&document.buffer, selection.cursor)
            } else {
                selection.cursor
            };
            let boundary = buffer_next_word_start(&document.buffer, start);
            if normal {
                selection.anchor = start;
            }
            selection.cursor = if boundary > start {
                buffer_previous_char(&document.buffer, boundary)
            } else {
                start
            };
        } else {
            let origin = selection.cursor;
            let target = buffer_previous_word_start(&document.buffer, origin);
            if normal {
                let current_word_start = buffer_previous_word_start(
                    &document.buffer,
                    buffer_next_char(&document.buffer, origin),
                );
                selection.anchor = if current_word_start == target {
                    origin
                } else {
                    buffer_previous_char(&document.buffer, origin)
                };
            }
            selection.cursor = target;
        }
        if selection.cursor == buffer_len(&document.buffer) && selection.cursor > 0 {
            selection.cursor = buffer_previous_char(&document.buffer, selection.cursor);
        }
        if normal && selection.anchor == buffer_len(&document.buffer) {
            selection.anchor = selection.cursor;
        }
    }
    document_set_selections(document, &selections);
    document.preferred_column = buffer_line_and_column(&document.buffer, document.cursor).1;
}

fn editor_add_cursor_below(editor: &mut Editor) {
    let document = editor_document_mut(editor);
    let primary = SelectionState {
        cursor: document.cursor,
        anchor: document.anchor,
    };
    let (line, column) = buffer_line_and_column(&document.buffer, primary.cursor);
    if line + 1 >= buffer_line_count(&document.buffer) {
        return;
    }
    let cursor = command_cursor_clamped(
        &document.buffer,
        buffer_position_at_line_column(&document.buffer, line + 1, column),
        Mode::Normal,
    );
    if document
        .secondary_selections
        .iter()
        .any(|selection| selection.cursor == cursor)
    {
        return;
    }
    document.secondary_selections.push(primary);
    document.cursor = cursor;
    document.anchor = cursor;
}

fn editor_insert_newline(editor: &mut Editor) {
    let auto_indentation = editor.config.flags.contains(EditorFlags::AUTO_INDENTATION);
    let scope_indentation = editor
        .config
        .flags
        .contains(EditorFlags::AUTO_INDENT_SCOPES);
    let indentation_spaces = editor.config.indentation_spaces;
    let document = editor_document_mut(editor);
    let temp = idno_std::mem().scratch().temp();
    let mut replacements = temp.vec(document.insertion_points.len());
    let mut inserted_bytes = temp.vec(document.insertion_points.len() * (indentation_spaces + 1));
    let mut indentation = temp.vec(indentation_spaces);
    let mut outer_indentation = temp.vec(indentation_spaces);
    let mut replacement_starts = temp.vec(document.insertion_points.len());
    let mut cursor_offsets = temp.vec(document.insertion_points.len());
    for &position in &document.insertion_points {
        let line_start = buffer_line_start(&document.buffer, position);
        let indentation_only_before_cursor = line_start < position
            && (line_start..position).all(|byte_position| {
                matches!(buffer_byte(&document.buffer, byte_position), b' ' | b'\t')
            });
        replacement_starts.push(if indentation_only_before_cursor {
            line_start
        } else {
            position
        });
        indentation.clear();
        if auto_indentation {
            indentation_for_position(
                &document.buffer,
                position,
                scope_indentation,
                indentation_spaces,
                syntax_highlighting_spans(&document.syntax),
                &mut indentation,
            );
        }
        let inserted_start = inserted_bytes.len();
        let paired = position > 0
            && position < buffer_len(&document.buffer)
            && matches!(
                (
                    buffer_byte(&document.buffer, position - 1),
                    buffer_byte(&document.buffer, position)
                ),
                (b'(', b')') | (b'[', b']') | (b'{', b'}')
            );
        inserted_bytes.push(b'\n');
        inserted_bytes.extend_from_slice(&indentation);
        cursor_offsets.push(1 + indentation.len());
        if paired {
            outer_indentation.clear();
            if auto_indentation {
                indentation_for_position(
                    &document.buffer,
                    position,
                    false,
                    indentation_spaces,
                    syntax_highlighting_spans(&document.syntax),
                    &mut outer_indentation,
                );
            }
            inserted_bytes.push(b'\n');
            inserted_bytes.extend_from_slice(&outer_indentation);
        }
        replacements.push(Replacement {
            start: if indentation_only_before_cursor {
                line_start
            } else {
                position
            },
            end: position,
            inserted: inserted_start..inserted_bytes.len(),
        });
    }
    document_replace_ranges(document, &mut replacements, &inserted_bytes, None);
    document.insertion_points.clear();
    for (&position, &cursor_offset) in replacement_starts.iter().zip(&cursor_offsets) {
        document
            .insertion_points
            .push(offset_after_replacements(position, false, &replacements) + cursor_offset);
    }
}

fn editor_open_line_below(editor: &mut Editor) {
    let auto_indentation = editor.config.flags.contains(EditorFlags::AUTO_INDENTATION);
    let scope_indentation = editor
        .config
        .flags
        .contains(EditorFlags::AUTO_INDENT_SCOPES);
    let indentation_spaces = editor.config.indentation_spaces;
    let document = editor_document_mut(editor);
    let temp = idno_std::mem().scratch().temp();
    let mut selections = temp.vec(document.secondary_selections.len() + 1);
    document_selections(document, &mut selections);
    document.insertion_points.clear();
    document.insertion_points.reserve(selections.len());
    document.insertion_points.extend(
        selections
            .iter()
            .map(|selection| buffer_line_end(&document.buffer, selection.cursor)),
    );
    let mut replacements = temp.vec(document.insertion_points.len());
    let mut inserted_bytes = temp.vec(document.insertion_points.len() * (indentation_spaces + 1));
    let mut indentation = temp.vec(indentation_spaces);
    for &position in &document.insertion_points {
        indentation.clear();
        if auto_indentation {
            indentation_for_position(
                &document.buffer,
                position,
                scope_indentation,
                indentation_spaces,
                syntax_highlighting_spans(&document.syntax),
                &mut indentation,
            );
        }
        let inserted_start = inserted_bytes.len();
        inserted_bytes.push(b'\n');
        inserted_bytes.extend_from_slice(&indentation);
        replacements.push(Replacement {
            start: position,
            end: position,
            inserted: inserted_start..inserted_bytes.len(),
        });
    }
    replacements.sort_unstable_by_key(|replacement| replacement.start);
    replacements.dedup_by(|right, left| right.start == left.start && right.end == left.end);
    let mut after = temp.vec(document.insertion_points.len());
    let mut transformed_insertion_points = temp.vec(document.insertion_points.len());
    for &position in &document.insertion_points {
        let position = offset_after_replacements(position, true, &replacements);
        transformed_insertion_points.push(position);
        after.push(SelectionState {
            cursor: position,
            anchor: position,
        });
    }
    document_replace_ranges(document, &mut replacements, &inserted_bytes, Some(&after));
    document.insertion_points.clear();
    document
        .insertion_points
        .extend_from_slice(&transformed_insertion_points);
    editor.mode = Mode::Insert;
}

fn editor_open_line_above(editor: &mut Editor) {
    let auto_indentation = editor.config.flags.contains(EditorFlags::AUTO_INDENTATION);
    let indentation_spaces = editor.config.indentation_spaces;
    let document = editor_document_mut(editor);
    let temp = idno_std::mem().scratch().temp();
    let mut selections = temp.vec(document.secondary_selections.len() + 1);
    document_selections(document, &mut selections);
    document.insertion_points.clear();
    document.insertion_points.reserve(selections.len());
    document.insertion_points.extend(
        selections
            .iter()
            .map(|selection| buffer_line_start(&document.buffer, selection.cursor)),
    );
    let mut replacements = temp.vec(document.insertion_points.len());
    let mut inserted_bytes = temp.vec(document.insertion_points.len() * (indentation_spaces + 1));
    let mut indentation = temp.vec(indentation_spaces);
    for &position in &document.insertion_points {
        indentation.clear();
        if auto_indentation {
            indentation_for_position(
                &document.buffer,
                position,
                false,
                indentation_spaces,
                syntax_highlighting_spans(&document.syntax),
                &mut indentation,
            );
        }
        let inserted_start = inserted_bytes.len();
        inserted_bytes.extend_from_slice(&indentation);
        inserted_bytes.push(b'\n');
        replacements.push(Replacement {
            start: position,
            end: position,
            inserted: inserted_start..inserted_bytes.len(),
        });
    }
    replacements.sort_unstable_by_key(|replacement| replacement.start);
    replacements.dedup_by(|right, left| right.start == left.start && right.end == left.end);
    let mut after = temp.vec(document.insertion_points.len());
    let mut transformed_insertion_points = temp.vec(document.insertion_points.len());
    for &position in &document.insertion_points {
        let position = offset_after_replacements(position, true, &replacements).saturating_sub(1);
        transformed_insertion_points.push(position);
        after.push(SelectionState {
            cursor: position,
            anchor: position,
        });
    }
    document_replace_ranges(document, &mut replacements, &inserted_bytes, Some(&after));
    document.insertion_points.clear();
    document
        .insertion_points
        .extend_from_slice(&transformed_insertion_points);
    editor.mode = Mode::Insert;
}

fn editor_insert_blank_lines(editor: &mut Editor, below: bool) {
    profiling::function_scope!();
    let document = editor_document_mut(editor);
    let temp = idno_std::mem().scratch().temp();
    let mut selections = temp.vec(document.secondary_selections.len() + 1);
    document_selections(document, &mut selections);
    let mut replacements = temp.vec(selections.len());
    for selection in &selections {
        let position = if below {
            buffer_line_end(&document.buffer, selection.cursor)
        } else {
            buffer_line_start(&document.buffer, selection.cursor)
        };
        replacements.push(Replacement {
            start: position,
            end: position,
            inserted: 0..1,
        });
    }
    replacements.sort_unstable_by_key(|replacement| replacement.start);
    replacements.dedup_by_key(|replacement| replacement.start);
    let mut after = temp.vec(selections.len());
    for selection in selections {
        after.push(SelectionState {
            cursor: offset_after_replacements(selection.cursor, true, &replacements),
            anchor: offset_after_replacements(selection.anchor, true, &replacements),
        });
    }
    document_replace_ranges(document, &mut replacements, b"\n", Some(&after));
}

fn editor_move_insert_points_horizontal(editor: &mut Editor, forward: bool) {
    let document = editor_document_mut(editor);
    for position in &mut document.insertion_points {
        *position = if forward {
            buffer_next_char(&document.buffer, *position)
        } else {
            buffer_previous_char(&document.buffer, *position)
        };
    }
}

fn editor_move_insert_points_vertical(editor: &mut Editor, down: bool) {
    let document = editor_document_mut(editor);
    for position in &mut document.insertion_points {
        let (line, column) = buffer_line_and_column(&document.buffer, *position);
        let target = if down {
            line.saturating_add(1)
        } else {
            line.saturating_sub(1)
        };
        *position = buffer_position_at_line_column(&document.buffer, target, column);
    }
}

fn editor_move_insert_points_to_boundary(editor: &mut Editor, end: bool) {
    let document = editor_document_mut(editor);
    for position in &mut document.insertion_points {
        *position = if end {
            buffer_line_end(&document.buffer, *position)
        } else {
            buffer_line_start(&document.buffer, *position)
        };
    }
}

fn indentation_for_position(
    buffer: &GapBuffer,
    position: usize,
    scope_indentation: bool,
    indentation_spaces: usize,
    syntax_spans: &[SyntaxSpan],
    indentation: &mut Vec<u8, impl Allocator>,
) {
    profiling::function_scope!();
    let start = buffer_line_start(buffer, position);
    let end = buffer_line_end(buffer, position);
    indentation.clear();
    let mut at = start;
    while at < end && matches!(buffer_byte(buffer, at), b' ' | b'\t') {
        indentation.push(buffer_byte(buffer, at));
        at += 1;
    }
    if !scope_indentation {
        return;
    }

    let mut round_depth = 0usize;
    let mut square_depth = 0usize;
    let mut curly_depth = 0usize;
    let mut at = position.min(buffer_len(buffer));
    let mut span_position = syntax_spans.partition_point(|span| (span.start as usize) < at);
    while at > 0 {
        let byte_position = at - 1;
        while span_position > 0 && syntax_spans[span_position - 1].start as usize > byte_position {
            span_position -= 1;
        }
        if span_position > 0 {
            let span = syntax_spans[span_position - 1];
            if span.start as usize <= byte_position
                && byte_position < span.end as usize
                && matches!(
                    span.kind,
                    SyntaxKind::Comment
                        | SyntaxKind::String
                        | SyntaxKind::Markup
                        | SyntaxKind::CommentAnnotation
                        | SyntaxKind::CommentNote
                        | SyntaxKind::CommentWarning
                        | SyntaxKind::CommentError
                )
            {
                at = span.start as usize;
                span_position -= 1;
                continue;
            }
        }
        let byte = buffer_byte(buffer, byte_position);
        let unmatched_opening = match byte {
            b')' => {
                round_depth += 1;
                false
            }
            b']' => {
                square_depth += 1;
                false
            }
            b'}' => {
                curly_depth += 1;
                false
            }
            b'(' if round_depth > 0 => {
                round_depth -= 1;
                false
            }
            b'[' if square_depth > 0 => {
                square_depth -= 1;
                false
            }
            b'{' if curly_depth > 0 => {
                curly_depth -= 1;
                false
            }
            b'(' | b'[' | b'{' => true,
            _ => false,
        };
        if unmatched_opening {
            let scope_start = buffer_line_start(buffer, byte_position);
            let mut whitespace = scope_start;
            while whitespace < byte_position
                && matches!(buffer_byte(buffer, whitespace), b' ' | b'\t')
            {
                whitespace += 1;
            }
            let scope_indentation_length = whitespace - scope_start + indentation_spaces;
            let mut last = position.min(end);
            while last > start && matches!(buffer_byte(buffer, last - 1), b' ' | b'\t') {
                last -= 1;
            }
            let line_continues =
                last > start && !matches!(buffer_byte(buffer, last - 1), b';' | b'{' | b'}');
            if line_continues && indentation.len() > scope_indentation_length {
                return;
            }
            indentation.clear();
            for position in scope_start..whitespace {
                indentation.push(buffer_byte(buffer, position));
            }
            indentation.extend(std::iter::repeat_n(b' ', indentation_spaces));
            return;
        }
        at = byte_position;
    }
}

fn editor_select_surrounding(editor: &mut Editor, delimiter: char, inside: bool) {
    if !delimiter.is_ascii() {
        return;
    }
    let (opening, closing) = delimiter_pair(delimiter as u8);
    let document = editor_document_mut(editor);
    let temp = idno_std::mem().scratch().temp();
    let mut selections = temp.vec(document.secondary_selections.len() + 1);
    document_selections(document, &mut selections);
    for selection in &mut selections {
        let Some((start, end)) =
            surrounding_range(&document.buffer, selection.cursor, opening, closing)
        else {
            continue;
        };
        selection.anchor = if inside {
            buffer_next_char(&document.buffer, start)
        } else {
            start
        };
        selection.cursor = if inside {
            buffer_previous_char(&document.buffer, end)
        } else {
            end
        };
    }
    document_set_selections(document, &selections);
}

fn editor_goto_argument(editor: &mut Editor, forward: bool) {
    profiling::function_scope!();
    let document = editor_document_mut(editor);
    let temp = idno_std::mem().scratch().temp();
    let mut selections = temp.vec(document.secondary_selections.len() + 1);
    let mut arguments = temp.vec(16);
    document_selections(document, &mut selections);
    for selection in &mut selections {
        let mut surrounding = None;
        for (opening, closing) in [(b'(', b')'), (b'[', b']'), (b'{', b'}')] {
            if let Some(range) =
                surrounding_range(&document.buffer, selection.cursor, opening, closing)
                && surrounding
                    .is_none_or(|current: (usize, usize)| range.1 - range.0 < current.1 - current.0)
            {
                surrounding = Some(range);
            }
        }
        if surrounding.is_none() && forward {
            let mut position = selection.cursor;
            while position < buffer_len(&document.buffer) {
                let byte = buffer_byte(&document.buffer, position);
                let closing = match byte {
                    b'(' => b')',
                    b'[' => b']',
                    b'{' => b'}',
                    _ => {
                        position += 1;
                        continue;
                    }
                };
                surrounding = surrounding_range(&document.buffer, position, byte, closing);
                break;
            }
        }
        let Some((open, close)) = surrounding else {
            continue;
        };
        rust_argument_ranges(&document.buffer, open, close, &mut arguments);
        let target = if forward {
            arguments
                .iter()
                .find(|range| range.start > selection.cursor)
                .or_else(|| {
                    arguments
                        .iter()
                        .find(|range| range.contains(&selection.cursor))
                        .and_then(|current| {
                            arguments.iter().find(|range| range.start > current.start)
                        })
                })
        } else {
            arguments
                .iter()
                .rev()
                .find(|range| range.end.saturating_sub(1) < selection.cursor)
        };
        let Some(target) = target else {
            continue;
        };
        selection.anchor = target.start;
        selection.cursor = target.end.saturating_sub(1);
    }
    document_set_selections(document, &selections);
}

fn rust_argument_ranges(
    buffer: &GapBuffer,
    open: usize,
    close: usize,
    result: &mut Vec<std::ops::Range<usize>, impl Allocator>,
) {
    profiling::function_scope!();
    result.clear();
    let mut start = open + 1;
    let mut position = start;
    let mut round = 0usize;
    let mut square = 0usize;
    let mut curly = 0usize;
    let mut angle = 0usize;
    while position <= close {
        let byte = if position < close {
            buffer_byte(buffer, position)
        } else {
            b','
        };
        let separator = byte == b',' && round == 0 && square == 0 && curly == 0 && angle == 0;
        if separator {
            let mut argument_start = start;
            let mut argument_end = position;
            while argument_start < argument_end
                && buffer_byte(buffer, argument_start).is_ascii_whitespace()
            {
                argument_start += 1;
            }
            while argument_end > argument_start
                && buffer_byte(buffer, argument_end - 1).is_ascii_whitespace()
            {
                argument_end -= 1;
            }
            if argument_start < argument_end {
                result.push(argument_start..argument_end);
            }
            start = position + 1;
        } else {
            match byte {
                b'(' => round += 1,
                b')' => round = round.saturating_sub(1),
                b'[' => square += 1,
                b']' => square = square.saturating_sub(1),
                b'{' => curly += 1,
                b'}' => curly = curly.saturating_sub(1),
                b'<' => angle += 1,
                b'>' => angle = angle.saturating_sub(1),
                _ => {}
            }
        }
        position += 1;
    }
}

fn editor_add_surround(editor: &mut Editor, delimiter: char) {
    profiling::function_scope!();
    if !delimiter.is_ascii() {
        return;
    }
    let (opening, closing) = delimiter_pair(delimiter as u8);
    let inserted = [opening, closing];
    let document = editor_document_mut(editor);
    let temp = idno_std::mem().scratch().temp();
    let mut selections = temp.vec(document.secondary_selections.len() + 1);
    let mut replacements = temp.vec((document.secondary_selections.len() + 1) * 2);
    document_selections(document, &mut selections);
    for selection in &selections {
        let start = selection.anchor.min(selection.cursor);
        let end = buffer_next_char(&document.buffer, selection.anchor.max(selection.cursor));
        replacements.push(Replacement {
            start,
            end: start,
            inserted: 0..1,
        });
        replacements.push(Replacement {
            start: end,
            end,
            inserted: 1..2,
        });
    }
    replacements.sort_unstable_by_key(|replacement| replacement.start);
    let mut after = temp.vec(selections.len());
    for selection in &selections {
        after.push(SelectionState {
            anchor: offset_after_replacements(selection.anchor, true, &replacements),
            cursor: offset_after_replacements(selection.cursor, true, &replacements),
        });
    }
    document_replace_ranges(document, &mut replacements, &inserted, Some(&after));
}

fn editor_replace_surround(editor: &mut Editor, from: char, to: char) {
    profiling::function_scope!();
    if !from.is_ascii() || !to.is_ascii() {
        return;
    }
    let (from_opening, from_closing) = delimiter_pair(from as u8);
    let (to_opening, to_closing) = delimiter_pair(to as u8);
    let inserted = [to_opening, to_closing];
    let document = editor_document_mut(editor);
    let temp = idno_std::mem().scratch().temp();
    let mut selections = temp.vec(document.secondary_selections.len() + 1);
    let mut replacements = temp.vec((document.secondary_selections.len() + 1) * 2);
    document_selections(document, &mut selections);
    for selection in &selections {
        let Some((start, end)) = surrounding_range(
            &document.buffer,
            selection.cursor,
            from_opening,
            from_closing,
        ) else {
            continue;
        };
        replacements.push(Replacement {
            start,
            end: start + 1,
            inserted: 0..1,
        });
        replacements.push(Replacement {
            start: end,
            end: end + 1,
            inserted: 1..2,
        });
    }
    document_replace_ranges(document, &mut replacements, &inserted, Some(&selections));
}

#[derive(Clone, Copy)]
struct FunctionRange {
    start: usize,
    open: usize,
    close: usize,
}

fn editor_select_enclosing_function(editor: &mut Editor, inside: bool) {
    profiling::function_scope!();
    let document = editor_document_mut(editor);
    code_index_step(
        &document.buffer,
        &mut document.code_index,
        usize::MAX,
        std::time::Duration::from_millis(2),
    );
    let temp = idno_std::mem().scratch().temp();
    let mut functions = temp.vec(64);
    rust_function_ranges(document, &mut functions);
    let target = functions
        .iter()
        .filter(|function| function.start <= document.cursor && document.cursor <= function.close)
        .max_by_key(|function| function.close - function.start)
        .copied();
    let Some(target) = target else {
        editor.status.push_str("no enclosing function");
        return;
    };
    let (start, end) = if inside {
        (
            buffer_next_char(&document.buffer, target.open),
            target.close,
        )
    } else {
        (
            target.start,
            buffer_next_char(&document.buffer, target.close),
        )
    };
    editor_set_symbol_selection(editor, start, end);
}

fn editor_goto_function(editor: &mut Editor, forward: bool) {
    profiling::function_scope!();
    let document = editor_document_mut(editor);
    code_index_step(
        &document.buffer,
        &mut document.code_index,
        usize::MAX,
        std::time::Duration::from_millis(2),
    );
    let temp = idno_std::mem().scratch().temp();
    let mut functions = temp.vec(64);
    rust_function_ranges(document, &mut functions);
    if functions.is_empty() {
        editor.status.push_str("no functions");
        return;
    }
    functions.sort_unstable_by_key(|function| function.start);
    let cursor = document.cursor;
    let target = if forward {
        functions
            .iter()
            .find(|function| function.start > cursor)
            .unwrap_or(&functions[0])
    } else {
        functions
            .iter()
            .rev()
            .find(|function| function.start < cursor)
            .unwrap_or(&functions[functions.len() - 1])
    };
    let start = target.start;
    let end = buffer_next_char(&document.buffer, target.close);
    editor_select_symbol(editor, start, end);
}

fn rust_function_ranges(document: &Document, result: &mut Vec<FunctionRange, impl Allocator>) {
    profiling::function_scope!();
    result.clear();
    for symbol in &document.code_index.symbols {
        if symbol.kind != crate::code_index::CodeSymbolKind::Function {
            continue;
        }
        let identifier = document.code_index.identifiers[symbol.identifier as usize];
        let mut open = identifier.end as usize;
        while open < buffer_len(&document.buffer)
            && !matches!(buffer_byte(&document.buffer, open), b'{' | b';')
        {
            open += 1;
        }
        if open >= buffer_len(&document.buffer) || buffer_byte(&document.buffer, open) != b'{' {
            continue;
        }
        let Some(close) = rust_matching_body_brace(&document.buffer, open) else {
            continue;
        };
        let line_start = buffer_line_start(&document.buffer, identifier.start as usize);
        let mut start = line_start;
        while start < identifier.start as usize
            && matches!(buffer_byte(&document.buffer, start), b' ' | b'\t')
        {
            start += 1;
        }
        result.push(FunctionRange { start, open, close });
    }
}

fn rust_matching_body_brace(buffer: &GapBuffer, open: usize) -> Option<usize> {
    let length = buffer_len(buffer);
    let mut position = open + 1;
    let mut depth = 0usize;
    let mut string = 0u8;
    let mut escaped = false;
    let mut line_comment = false;
    let mut block_depth = 0usize;
    while position < length {
        let byte = buffer_byte(buffer, position);
        let next = (position + 1 < length).then(|| buffer_byte(buffer, position + 1));
        if line_comment {
            line_comment = byte != b'\n';
        } else if block_depth > 0 {
            if byte == b'/' && next == Some(b'*') {
                block_depth += 1;
                position += 1;
            } else if byte == b'*' && next == Some(b'/') {
                block_depth -= 1;
                position += 1;
            }
        } else if string != 0 {
            if byte == b'\\' && !escaped {
                escaped = true;
            } else {
                if byte == string && !escaped {
                    string = 0;
                }
                escaped = false;
            }
        } else if byte == b'/' && next == Some(b'/') {
            line_comment = true;
            position += 1;
        } else if byte == b'/' && next == Some(b'*') {
            block_depth = 1;
            position += 1;
        } else if byte == b'\'' {
            let mut lifetime_end = position + 1;
            if lifetime_end < length && rust_identifier_byte(buffer_byte(buffer, lifetime_end)) {
                lifetime_end += 1;
                while lifetime_end < length
                    && rust_identifier_byte(buffer_byte(buffer, lifetime_end))
                {
                    lifetime_end += 1;
                }
            }
            if lifetime_end > position + 1
                && (lifetime_end >= length || buffer_byte(buffer, lifetime_end) != b'\'')
            {
                position = lifetime_end;
                continue;
            }
            string = byte;
        } else if byte == b'"' {
            string = byte;
        } else if byte == b'{' {
            depth += 1;
        } else if byte == b'}' {
            if depth == 0 {
                return Some(position);
            }
            depth -= 1;
        }
        position += 1;
    }
    None
}

fn delimiter_pair(delimiter: u8) -> (u8, u8) {
    match delimiter {
        b'(' | b')' => (b'(', b')'),
        b'[' | b']' => (b'[', b']'),
        b'{' | b'}' => (b'{', b'}'),
        _ => (delimiter, delimiter),
    }
}

fn surrounding_range(
    buffer: &GapBuffer,
    cursor: usize,
    opening: u8,
    closing: u8,
) -> Option<(usize, usize)> {
    if opening == closing {
        let start = match (0..=cursor.min(buffer_len(buffer).saturating_sub(1)))
            .rev()
            .find(|&position| buffer_byte(buffer, position) == opening)
        {
            Some(start) => start,
            None => return None,
        };
        let end = match (cursor.saturating_add(1)..buffer_len(buffer))
            .find(|&position| buffer_byte(buffer, position) == closing)
        {
            Some(end) => end,
            None => return None,
        };
        return Some((start, end));
    }
    let mut depth = 0;
    let mut start = None;
    for position in (0..=cursor.min(buffer_len(buffer).saturating_sub(1))).rev() {
        let byte = buffer_byte(buffer, position);
        if byte == closing {
            depth += 1;
        } else if byte == opening {
            if depth == 0 {
                start = Some(position);
                break;
            }
            depth -= 1;
        }
    }
    let start = match start {
        Some(start) => start,
        None => return None,
    };
    depth = 0;
    for position in start + 1..buffer_len(buffer) {
        let byte = buffer_byte(buffer, position);
        if byte == opening {
            depth += 1;
        } else if byte == closing {
            if depth == 0 {
                return Some((start, position));
            }
            depth -= 1;
        }
    }
    None
}

fn editor_move_horizontal(editor: &mut Editor, forward: bool) {
    let mode = editor.mode;
    let document = editor_document_mut(editor);
    let temp = idno_std::mem().scratch().temp();
    let mut selections = temp.vec(document.secondary_selections.len() + 1);
    document_selections(document, &mut selections);
    for selection in &mut selections {
        selection.cursor = if forward {
            buffer_next_char(&document.buffer, selection.cursor)
        } else {
            buffer_previous_char(&document.buffer, selection.cursor)
        };
        selection.cursor = command_cursor_clamped(&document.buffer, selection.cursor, mode);
        if mode == Mode::Normal {
            selection.anchor = selection.cursor;
        }
    }
    document_set_selections(document, &selections);
    document.preferred_column = buffer_line_and_column(&document.buffer, document.cursor).1;
}

fn editor_move_vertical(editor: &mut Editor, down: bool) {
    let mode = editor.mode;
    let document = editor_document_mut(editor);
    let line = buffer_line_and_column(&document.buffer, document.cursor).0;
    let primary_line = line;
    let temp = idno_std::mem().scratch().temp();
    let mut selections = temp.vec(document.secondary_selections.len() + 1);
    document_selections(document, &mut selections);
    for selection in &mut selections {
        let (selection_line, selection_column) =
            buffer_line_and_column(&document.buffer, selection.cursor);
        let target_line = if down {
            selection_line.saturating_add(1)
        } else {
            selection_line.saturating_sub(1)
        };
        selection.cursor = buffer_position_at_line_column(
            &document.buffer,
            target_line,
            if selection_line == primary_line {
                document.preferred_column
            } else {
                selection_column
            },
        );
        let target_start = buffer_line_start(&document.buffer, selection.cursor);
        let target_end = buffer_line_end(&document.buffer, selection.cursor);
        if mode != Mode::Insert && selection.cursor == target_end && target_end > target_start {
            selection.cursor = buffer_previous_char(&document.buffer, target_end);
        }
        selection.cursor = command_cursor_clamped(&document.buffer, selection.cursor, mode);
        if mode == Mode::Normal {
            selection.anchor = selection.cursor;
        }
    }
    document_set_selections(document, &selections);
}

fn editor_goto_file_start(editor: &mut Editor) {
    editor_goto_absolute_position(editor, 0);
}

fn editor_goto_line(editor: &mut Editor, line: usize) {
    profiling::function_scope!();
    let document = editor_document(editor);
    let position = buffer_position_at_line_column(&document.buffer, line, 0);
    editor_goto_absolute_position(editor, position);
}

fn editor_goto_git_change(editor: &mut Editor, forward: bool) {
    profiling::function_scope!();
    let target = {
        let document = editor_document(editor);
        let line = buffer_line_and_column(&document.buffer, document.cursor).0;
        git_gutter_next_change(&document.git_gutter, line, forward)
    };
    let Some(target) = target else {
        editor.status.push_str("no Git changes");
        return;
    };
    editor_goto_line(editor, target);
}

fn editor_next_diagnostic(editor: &mut Editor, forward: bool) {
    profiling::function_scope!();
    if editor.diagnostics.published.is_empty() {
        editor
            .status
            .push_str(if diagnostics_pending(&editor.diagnostics) {
                "compiler diagnostics are still loading"
            } else {
                "no diagnostics"
            });
        return;
    }
    let Some(path) = editor_document(editor).path.as_ref() else {
        editor.status.push_str("buffer has no diagnostics");
        return;
    };
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
    let temp = idno_std::mem().scratch().temp();
    let mut candidates = temp.vec(32);
    for (index, diagnostic) in editor.diagnostics.published.iter().enumerate() {
        let diagnostic_path =
            std::fs::canonicalize(&diagnostic.path).unwrap_or_else(|_| diagnostic.path.clone());
        if diagnostic_path == path {
            candidates.push(index);
        }
    }
    if candidates.is_empty() {
        editor.status.push_str("buffer has no diagnostics");
        return;
    }
    let document = editor_document(editor);
    let current = candidates.iter().position(|&index| {
        let diagnostic = &editor.diagnostics.published[index];
        buffer_position_at_line_column(
            &document.buffer,
            diagnostic.line as usize,
            diagnostic.column as usize,
        ) == document.cursor
    });
    let target = match current {
        Some(current) if forward => candidates[(current + 1) % candidates.len()],
        Some(current) => candidates[current.checked_sub(1).unwrap_or(candidates.len() - 1)],
        None => candidates[0],
    };
    editor_goto_diagnostic(editor, target);
}

fn editor_goto_diagnostic(editor: &mut Editor, diagnostic: usize) {
    profiling::function_scope!();
    let Some(diagnostic) = editor.diagnostics.published.get(diagnostic) else {
        return;
    };
    let path = diagnostic.path.clone();
    let diagnostic_range = (
        diagnostic.line as usize,
        diagnostic.column as usize,
        diagnostic.end_line as usize,
        diagnostic.end_column as usize,
    );
    let before = editor_location(editor);
    let Some(target) = editor_document_target(editor, path) else {
        return;
    };
    editor_switch_document_state(editor, target);
    let document = editor_document(editor);
    let start = buffer_position_at_line_character_column(
        &document.buffer,
        diagnostic_range.0,
        diagnostic_range.1,
    );
    let mut end = buffer_position_at_line_character_column(
        &document.buffer,
        diagnostic_range.2,
        diagnostic_range.3,
    );
    if end <= start {
        end = buffer_next_char(&document.buffer, start);
    }
    editor_set_symbol_selection(editor, start, end);
    editor_center_view(editor);
    let after = editor_location(editor);
    editor_record_jump(editor, before, after);
}

fn buffer_position_at_line_character_column(
    buffer: &GapBuffer,
    line: usize,
    column: usize,
) -> usize {
    profiling::function_scope!();
    let mut position = buffer_position_at_line_column(buffer, line, 0);
    let end = buffer_line_end(buffer, position);
    for _ in 0..column {
        if position >= end {
            break;
        }
        position = buffer_next_char(buffer, position);
    }
    position
}

fn editor_goto_definition(editor: &mut Editor) {
    profiling::function_scope!();
    if !editor_document(editor).code_index.enabled {
        editor
            .status
            .push_str("definition provider unavailable for this language");
        return;
    }
    editor_finish_workspace_refresh(editor);
    let temp = idno_std::mem().scratch().temp();
    let mut name = temp.vec(64);
    let mut owner = temp.vec(64);
    let mut glob_owners = temp.vec(64);
    let (local_target, module_target, qualified_path, on_module_definition, follow_type_alias) = {
        let document = editor_document_mut(editor);
        while !document.code_index.complete {
            code_index_step(
                &document.buffer,
                &mut document.code_index,
                256 * 1024,
                std::time::Duration::MAX,
            );
        }
        let Some(identifier_index) =
            code_index_identifier_at(&document.code_index, document.cursor)
        else {
            editor.status.push_str("no indexed identifier");
            return;
        };
        let identifier_range = document.code_index.identifiers[identifier_index];
        for position in identifier_range.start as usize..identifier_range.end as usize {
            name.push(buffer_byte(&document.buffer, position));
        }
        rust_qualified_owner(
            &document.buffer,
            identifier_range.start as usize,
            &mut owner,
        );
        if owner.is_empty() {
            rust_import_owner(
                &document.buffer,
                &document.code_index,
                identifier_index,
                &mut owner,
            );
        }
        rust_glob_import_owners(
            &document.buffer,
            identifier_range.start as usize,
            &mut glob_owners,
        );
        let qualified_path =
            rust_path_separator_after(&document.buffer, identifier_range.end as usize);
        let target =
            code_index_definition_for(&document.buffer, &document.code_index, identifier_index)
                .map(|symbol| {
                    let symbol = document.code_index.symbols[symbol];
                    let identifier = document.code_index.identifiers[symbol.identifier as usize];
                    (
                        identifier.start as usize,
                        identifier.end as usize,
                        symbol.kind,
                    )
                });
        let module_target = code_index_definition_of_kind(
            &document.buffer,
            &document.code_index,
            identifier_index,
            CodeSymbolKind::Module,
        )
        .map(|symbol| {
            let symbol = document.code_index.symbols[symbol];
            let identifier = document.code_index.identifiers[symbol.identifier as usize];
            (identifier.start as usize, identifier.end as usize)
        });
        let on_module_definition = module_target.is_some_and(|(start, end)| {
            start == identifier_range.start as usize && end == identifier_range.end as usize
        });
        let follow_type_alias = target.is_some_and(|(start, end, kind)| {
            kind == CodeSymbolKind::Type
                && start == identifier_range.start as usize
                && end == identifier_range.end as usize
        }) && rust_type_alias_target(
            &document.buffer,
            identifier_range.end as usize,
            &mut owner,
            &mut name,
        );
        (
            target,
            module_target,
            qualified_path,
            on_module_definition,
            follow_type_alias,
        )
    };
    if let Some((start, end)) = module_target
        && (qualified_path
            || local_target.is_some_and(|(_, _, kind)| kind == CodeSymbolKind::Module))
    {
        if on_module_definition && editor_goto_module_file(editor, &name) {
            return;
        }
        editor_select_symbol(editor, start, end);
        return;
    }
    if let Some((start, end, kind)) = local_target
        && !follow_type_alias
        && (owner.is_empty() || kind == CodeSymbolKind::Module)
        && (!qualified_path || kind != crate::code_index::CodeSymbolKind::Value)
    {
        editor_select_symbol(editor, start, end);
        return;
    }
    if editor_goto_external_definition(editor, &owner, &glob_owners, &name) {
        return;
    }
    if rust_method_index_finish(&mut editor.rust_methods) {
        editor_refresh_completion(editor);
        if editor_goto_external_definition(editor, &owner, &glob_owners, &name) {
            return;
        }
    }
    while editor.project_discovery.is_some() {
        editor_poll_project_discovery(editor);
    }
    if rust_method_index_finish(&mut editor.rust_methods) {
        editor_refresh_completion(editor);
        if editor_goto_external_definition(editor, &owner, &glob_owners, &name) {
            return;
        }
    }
    editor_finish_project_search(editor);
    if editor_goto_external_definition(editor, &owner, &glob_owners, &name) {
        return;
    }
    editor.status.push_str("definition not found");
}

fn editor_fill_struct_fields(editor: &mut Editor) {
    profiling::function_scope!();
    if !editor_document(editor).code_index.enabled {
        editor.status.push_str("no Rust code action at selection");
        return;
    }
    let temp = idno_std::mem().scratch().temp();
    let mut brace_stack = temp.vec(32);
    let Some((open, close)) = rust_enclosing_braces(
        &editor_document(editor).buffer,
        editor_document(editor).cursor,
        &mut brace_stack,
    ) else {
        editor.status.push_str("no struct literal at selection");
        return;
    };
    let mut name_end = open;
    while name_end > 0
        && buffer_byte(&editor_document(editor).buffer, name_end - 1).is_ascii_whitespace()
    {
        name_end -= 1;
    }
    let mut name_start = name_end;
    while name_start > 0
        && rust_identifier_byte(buffer_byte(&editor_document(editor).buffer, name_start - 1))
    {
        name_start -= 1;
    }
    if name_start == name_end {
        editor.status.push_str("no struct literal at selection");
        return;
    }
    let mut owner = temp.vec(32);
    let mut owner_end = name_start;
    if owner_end >= 2
        && buffer_byte(&editor_document(editor).buffer, owner_end - 1) == b':'
        && buffer_byte(&editor_document(editor).buffer, owner_end - 2) == b':'
    {
        owner_end -= 2;
        let mut owner_start = owner_end;
        while owner_start > 0
            && rust_identifier_byte(buffer_byte(
                &editor_document(editor).buffer,
                owner_start - 1,
            ))
        {
            owner_start -= 1;
        }
        for position in owner_start..owner_end {
            owner.push(buffer_byte(&editor_document(editor).buffer, position));
        }
    }
    let mut name = temp.vec(name_end - name_start);
    for position in name_start..name_end {
        name.push(buffer_byte(&editor_document(editor).buffer, position));
    }
    if rust_method_index_finish(&mut editor.rust_methods) {
        editor_refresh_completion(editor);
    }
    let Some(symbol) = rust_indexed_symbol(&editor.rust_methods.corpus, &owner, &[], &name) else {
        editor.status.push_str("struct definition not indexed");
        return;
    };
    let definition = editor.rust_methods.corpus.symbols[symbol];
    let Some(path) = rust_symbol_path(&editor.rust_methods.corpus, symbol) else {
        editor.status.push_str("struct definition has no source");
        return;
    };
    let source = match std::fs::read(path) {
        Ok(source) => source,
        Err(error) => {
            write!(&mut editor.status, "cannot read struct definition: {error}").unwrap();
            return;
        }
    };
    let Some(declaration_open) = source[definition.end as usize..]
        .iter()
        .position(|&byte| byte == b'{')
        .map(|position| definition.end as usize + position)
    else {
        editor.status.push_str("definition is not a field struct");
        return;
    };
    let Some(declaration_close) = rust_matching_source_brace(&source, declaration_open) else {
        editor.status.push_str("incomplete struct definition");
        return;
    };
    let mut fields = temp.vec(32);
    rust_struct_field_ranges(&source, declaration_open, declaration_close, &mut fields);
    if fields.is_empty() {
        editor.status.push_str("struct has no named fields");
        return;
    }
    let document = editor_document(editor);
    let interior_is_empty = (open + 1..close)
        .all(|position| buffer_byte(&document.buffer, position).is_ascii_whitespace());
    if !interior_is_empty {
        editor
            .status
            .push_str("fill fields currently requires an empty literal");
        return;
    }
    let line_start = buffer_line_start(&document.buffer, open);
    let mut base_indentation = temp.vec(open - line_start);
    for position in line_start..open {
        let byte = buffer_byte(&document.buffer, position);
        if !matches!(byte, b' ' | b'\t') {
            base_indentation.clear();
            break;
        }
        base_indentation.push(byte);
    }
    let indentation_spaces = editor.config.indentation_spaces.max(1);
    let mut inserted = temp.vec(fields.len() * 32);
    for field in &fields {
        inserted.push(b'\n');
        inserted.extend_from_slice(&base_indentation);
        inserted.extend(std::iter::repeat_n(b' ', indentation_spaces));
        inserted.extend_from_slice(&source[field.start as usize..field.end as usize]);
        inserted.extend_from_slice(b": todo!(),");
    }
    inserted.push(b'\n');
    inserted.extend_from_slice(&base_indentation);
    let document = editor_document_mut(editor);
    let mut replacements = temp.vec(1);
    replacements.push(Replacement {
        start: open + 1,
        end: close,
        inserted: 0..inserted.len(),
    });
    document_replace_ranges(document, &mut replacements, &inserted, None);
    editor.status.push_str("filled struct fields");
}

fn rust_enclosing_braces(
    buffer: &GapBuffer,
    target: usize,
    stack: &mut Vec<usize, impl Allocator>,
) -> Option<(usize, usize)> {
    profiling::function_scope!();
    stack.clear();
    let length = buffer_len(buffer);
    let mut position = 0usize;
    while position < length {
        let byte = buffer_byte(buffer, position);
        if byte == b'{' {
            stack.push(position);
        } else if byte == b'}' {
            if position >= target
                && let Some(&open) = stack.last()
                && open <= target
            {
                return Some((open, position));
            }
            stack.pop();
        }
        position += 1;
    }
    None
}

fn rust_matching_source_brace(source: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (position, &byte) in source.iter().enumerate().skip(open) {
        if byte == b'{' {
            depth += 1;
        } else if byte == b'}' {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(position);
            }
        }
    }
    None
}

fn rust_struct_field_ranges(
    source: &[u8],
    open: usize,
    close: usize,
    fields: &mut Vec<std::ops::Range<u32>, impl Allocator>,
) {
    profiling::function_scope!();
    fields.clear();
    let mut position = open + 1;
    let mut depth = 1usize;
    while position < close {
        let byte = source[position];
        if byte == b'{' {
            depth += 1;
            position += 1;
            continue;
        }
        if byte == b'}' {
            depth = depth.saturating_sub(1);
            position += 1;
            continue;
        }
        if depth != 1 || !rust_identifier_byte(byte) || byte.is_ascii_digit() {
            position += 1;
            continue;
        }
        let start = position;
        position += 1;
        while position < close && rust_identifier_byte(source[position]) {
            position += 1;
        }
        let end = position;
        while position < close && source[position].is_ascii_whitespace() {
            position += 1;
        }
        if position < close
            && source[position] == b':'
            && source.get(position + 1) != Some(&b':')
            && end <= u32::MAX as usize
        {
            fields.push(start as u32..end as u32);
        }
    }
}

fn editor_goto_external_definition(
    editor: &mut Editor,
    owner: &[u8],
    glob_owners: &[u8],
    name: &[u8],
) -> bool {
    profiling::function_scope!();
    if let Some(primitive) = rust_primitive_document_symbol(name) {
        return editor_goto_indexed_rust_symbol(editor, &[], &[], primitive);
    }
    let method = {
        let document = editor_document(editor);
        rust_method_definition(
            &document.buffer,
            document.cursor,
            &editor.rust_methods.corpus,
        )
    };
    if let Some(method) = method {
        let definition = editor.rust_methods.corpus.methods[method];
        let Some(path) = rust_method_path(&editor.rust_methods.corpus, method) else {
            return false;
        };
        let path = path.to_path_buf();
        editor_navigate_to_symbol(
            editor,
            &path,
            definition.position as usize,
            definition.end as usize,
        );
        return true;
    }
    if editor_goto_indexed_rust_symbol(editor, owner, glob_owners, name) {
        return true;
    }
    if glob_owners.is_empty() && editor_goto_workspace_symbol(editor, owner, name) {
        return true;
    }
    false
}

fn rust_primitive_document_symbol(name: &[u8]) -> Option<&'static [u8]> {
    match name {
        b"bool" => Some(b"prim_bool"),
        b"char" => Some(b"prim_char"),
        b"str" => Some(b"prim_str"),
        b"i8" => Some(b"prim_i8"),
        b"i16" => Some(b"prim_i16"),
        b"i32" => Some(b"prim_i32"),
        b"i64" => Some(b"prim_i64"),
        b"i128" => Some(b"prim_i128"),
        b"isize" => Some(b"prim_isize"),
        b"u8" => Some(b"prim_u8"),
        b"u16" => Some(b"prim_u16"),
        b"u32" => Some(b"prim_u32"),
        b"u64" => Some(b"prim_u64"),
        b"u128" => Some(b"prim_u128"),
        b"usize" => Some(b"prim_usize"),
        b"f16" => Some(b"prim_f16"),
        b"f32" => Some(b"prim_f32"),
        b"f64" => Some(b"prim_f64"),
        b"f128" => Some(b"prim_f128"),
        _ => None,
    }
}

fn rust_type_alias_target(
    buffer: &GapBuffer,
    alias_end: usize,
    owner: &mut Vec<u8, impl Allocator>,
    name: &mut Vec<u8, impl Allocator>,
) -> bool {
    profiling::function_scope!();
    let length = buffer_len(buffer);
    let mut position = alias_end;
    let mut angle_depth = 0usize;
    loop {
        if position >= length {
            return false;
        }
        let byte = buffer_byte(buffer, position);
        if matches!(byte, b'\n' | b';' | b'{') {
            return false;
        }
        if byte == b'<' {
            angle_depth += 1;
        } else if byte == b'>' {
            angle_depth = angle_depth.saturating_sub(1);
        } else if byte == b'=' && angle_depth == 0 {
            position += 1;
            break;
        }
        position += 1;
    }

    owner.clear();
    name.clear();
    loop {
        while position < length && !rust_identifier_byte(buffer_byte(buffer, position)) {
            if matches!(buffer_byte(buffer, position), b'\n' | b';') {
                return false;
            }
            position += 1;
        }
        let start = position;
        while position < length && rust_identifier_byte(buffer_byte(buffer, position)) {
            position += 1;
        }
        if start == position {
            return false;
        }
        let ignored = [b"dyn".as_slice(), b"impl", b"mut", b"for"]
            .iter()
            .any(|word| {
                word.len() == position - start
                    && word
                        .iter()
                        .enumerate()
                        .all(|(offset, &byte)| buffer_byte(buffer, start + offset) == byte)
            });
        if !ignored {
            for byte_position in start..position {
                name.push(buffer_byte(buffer, byte_position));
            }
            break;
        }
    }

    loop {
        let mut separator = position;
        while separator < length && buffer_byte(buffer, separator).is_ascii_whitespace() {
            separator += 1;
        }
        if separator + 1 >= length
            || buffer_byte(buffer, separator) != b':'
            || buffer_byte(buffer, separator + 1) != b':'
        {
            return !name.is_empty();
        }
        position = separator + 2;
        while position < length && buffer_byte(buffer, position).is_ascii_whitespace() {
            position += 1;
        }
        let start = position;
        while position < length && rust_identifier_byte(buffer_byte(buffer, position)) {
            position += 1;
        }
        if start == position {
            return !name.is_empty();
        }
        owner.clear();
        owner.extend_from_slice(name);
        name.clear();
        for byte_position in start..position {
            name.push(buffer_byte(buffer, byte_position));
        }
    }
}

fn rust_path_separator_after(buffer: &GapBuffer, mut position: usize) -> bool {
    let length = buffer_len(buffer);
    while position < length && buffer_byte(buffer, position).is_ascii_whitespace() {
        position += 1;
    }
    position + 1 < length
        && buffer_byte(buffer, position) == b':'
        && buffer_byte(buffer, position + 1) == b':'
}

fn rust_qualified_owner(
    buffer: &GapBuffer,
    identifier_start: usize,
    owner: &mut Vec<u8, impl Allocator>,
) {
    owner.clear();
    let mut end = identifier_start;
    while end > 0 && buffer_byte(buffer, end - 1).is_ascii_whitespace() {
        end -= 1;
    }
    if end < 2 || buffer_byte(buffer, end - 1) != b':' || buffer_byte(buffer, end - 2) != b':' {
        return;
    }
    end -= 2;
    while end > 0 && buffer_byte(buffer, end - 1).is_ascii_whitespace() {
        end -= 1;
    }
    let mut start = end;
    while start > 0 && rust_identifier_byte(buffer_byte(buffer, start - 1)) {
        start -= 1;
    }
    for position in start..end {
        owner.push(buffer_byte(buffer, position));
    }
}

fn rust_import_owner(
    buffer: &GapBuffer,
    index: &CodeIndex,
    identifier: usize,
    owner: &mut Vec<u8, impl Allocator>,
) {
    profiling::function_scope!();
    owner.clear();
    let Some(target) = index.identifiers.get(identifier).copied() else {
        return;
    };
    if rust_import_owner_at(buffer, target.start as usize, owner) {
        return;
    }
    for candidate in &index.identifiers {
        if candidate.start == target.start
            || candidate.end - candidate.start != target.end - target.start
        {
            continue;
        }
        let equal = (0..target.end as usize - target.start as usize).all(|offset| {
            buffer_byte(buffer, candidate.start as usize + offset)
                == buffer_byte(buffer, target.start as usize + offset)
        });
        if equal && rust_import_owner_at(buffer, candidate.start as usize, owner) {
            return;
        }
    }
}

fn rust_import_owner_at(
    buffer: &GapBuffer,
    identifier_start: usize,
    owner: &mut Vec<u8, impl Allocator>,
) -> bool {
    profiling::function_scope!();
    let mut statement_start = identifier_start;
    while statement_start > 0 && buffer_byte(buffer, statement_start - 1) != b';' {
        statement_start -= 1;
    }
    let mut use_position = statement_start;
    while use_position < identifier_start && buffer_byte(buffer, use_position).is_ascii_whitespace()
    {
        use_position += 1;
    }
    if !buffer_range_matches_identifier(buffer, use_position, identifier_start, b"use") {
        return false;
    }

    let mut position = identifier_start;
    while position > statement_start && buffer_byte(buffer, position - 1).is_ascii_whitespace() {
        position -= 1;
    }
    if position >= statement_start + 2
        && buffer_byte(buffer, position - 1) == b':'
        && buffer_byte(buffer, position - 2) == b':'
    {
        position -= 2;
    } else {
        let mut brace_depth = 0usize;
        loop {
            if position <= statement_start {
                return false;
            }
            position -= 1;
            match buffer_byte(buffer, position) {
                b'}' => brace_depth += 1,
                b'{' if brace_depth > 0 => brace_depth -= 1,
                b'{' => break,
                _ => {}
            }
        }
        while position > statement_start && buffer_byte(buffer, position - 1).is_ascii_whitespace()
        {
            position -= 1;
        }
        if position < statement_start + 2
            || buffer_byte(buffer, position - 1) != b':'
            || buffer_byte(buffer, position - 2) != b':'
        {
            return false;
        }
        position -= 2;
    }
    while position > statement_start && buffer_byte(buffer, position - 1).is_ascii_whitespace() {
        position -= 1;
    }
    let end = position;
    while position > statement_start && rust_identifier_byte(buffer_byte(buffer, position - 1)) {
        position -= 1;
    }
    if position == end {
        return false;
    }
    owner.clear();
    for byte_position in position..end {
        owner.push(buffer_byte(buffer, byte_position));
    }
    true
}

fn rust_glob_import_owners(
    buffer: &GapBuffer,
    before: usize,
    owners: &mut Vec<u8, impl Allocator>,
) {
    profiling::function_scope!();
    owners.clear();
    let mut line_start = 0usize;
    while line_start < before {
        let line_end = buffer_line_end(buffer, line_start).min(before);
        let mut position = line_start;
        while position < line_end && buffer_byte(buffer, position).is_ascii_whitespace() {
            position += 1;
        }
        if position + 3 > line_end
            || !buffer_range_matches_identifier(buffer, position, line_end, b"use")
        {
            line_start = line_end.saturating_add(1);
            continue;
        }
        position += 3;
        let mut last_start = position;
        let mut last_end = position;
        let mut glob = false;
        while position < line_end {
            while position < line_end && buffer_byte(buffer, position).is_ascii_whitespace() {
                position += 1;
            }
            if position >= line_end {
                break;
            }
            if buffer_byte(buffer, position) == b'*' {
                glob = true;
                break;
            }
            let start = position;
            while position < line_end && rust_identifier_byte(buffer_byte(buffer, position)) {
                position += 1;
            }
            if start == position {
                break;
            }
            last_start = start;
            last_end = position;
            while position < line_end && buffer_byte(buffer, position).is_ascii_whitespace() {
                position += 1;
            }
            if position + 1 >= line_end
                || buffer_byte(buffer, position) != b':'
                || buffer_byte(buffer, position + 1) != b':'
            {
                break;
            }
            position += 2;
        }
        if glob && last_end > last_start {
            for byte_position in last_start..last_end {
                owners.push(buffer_byte(buffer, byte_position));
            }
            owners.push(0);
        }
        line_start = line_end.saturating_add(1);
    }
}

fn rust_owner_imported_by_glob(owners: &[u8], owner: &[u8]) -> bool {
    owners
        .split(|&byte| byte == 0)
        .any(|candidate| candidate == owner)
}

fn editor_goto_indexed_rust_symbol(
    editor: &mut Editor,
    owner: &[u8],
    glob_owners: &[u8],
    name: &[u8],
) -> bool {
    profiling::function_scope!();
    let symbol = rust_indexed_symbol(&editor.rust_methods.corpus, owner, glob_owners, name);
    let Some(symbol) = symbol else {
        return false;
    };
    let definition = editor.rust_methods.corpus.symbols[symbol];
    let Some(path) = rust_symbol_path(&editor.rust_methods.corpus, symbol) else {
        return false;
    };
    let path = path.to_path_buf();
    editor_navigate_to_symbol(
        editor,
        &path,
        definition.position as usize,
        definition.end as usize,
    );
    true
}

fn rust_indexed_symbol(
    corpus: &RustMethodCorpus,
    owner: &[u8],
    glob_owners: &[u8],
    name: &[u8],
) -> Option<usize> {
    profiling::function_scope!();
    let namespace_root = if owner.is_empty() {
        None
    } else {
        rust_namespace_root(corpus, owner)
    };
    corpus
        .symbols
        .iter()
        .enumerate()
        .filter_map(|(index, symbol)| {
            let candidate = &corpus.bytes[symbol.name_start as usize..symbol.name_end as usize];
            if candidate != name {
                return None;
            }
            let candidate_owner = rust_symbol_owner(corpus, index).as_bytes();
            let path = rust_symbol_path(corpus, index);
            let rank = if owner.is_empty()
                && glob_owners.is_empty()
                && rust_prelude_module(name).is_some_and(|module| {
                    path.is_some_and(|path| {
                        rust_standard_library_path(path) && rust_path_module_matches(path, module)
                    })
                }) {
                6
            } else if !owner.is_empty() && candidate_owner == owner {
                4
            } else if !candidate_owner.is_empty()
                && rust_owner_imported_by_glob(glob_owners, candidate_owner)
            {
                3
            } else if !owner.is_empty()
                && path.is_some_and(|path| rust_path_module_matches(path, owner))
            {
                2
            } else if !owner.is_empty()
                && candidate_owner.is_empty()
                && path
                    .is_some_and(|path| namespace_root.is_some_and(|root| path.starts_with(root)))
            {
                2
            } else if owner.is_empty() && glob_owners.is_empty() && candidate_owner.is_empty() {
                1
            } else {
                0
            };
            (rank > 0).then_some((rank, index))
        })
        .max_by_key(|&(rank, _)| rank)
        .map(|(_, index)| index)
}

fn rust_prelude_module(name: &[u8]) -> Option<&'static [u8]> {
    match name {
        b"Box" => Some(b"boxed"),
        b"Clone" => Some(b"clone"),
        b"Copy" => Some(b"marker"),
        b"Default" => Some(b"default"),
        b"Into" | b"From" | b"TryFrom" | b"TryInto" => Some(b"convert"),
        b"Iterator" | b"IntoIterator" => Some(b"iter"),
        b"None" | b"Some" | b"Option" => Some(b"option"),
        b"Err" | b"Ok" | b"Result" => Some(b"result"),
        b"String" => Some(b"string"),
        b"ToOwned" => Some(b"borrow"),
        b"Vec" => Some(b"vec"),
        _ => None,
    }
}

fn rust_standard_library_path(path: &std::path::Path) -> bool {
    profiling::function_scope!();
    let mut after_library = false;
    for component in path.components() {
        let bytes = component.as_os_str().as_encoded_bytes();
        if after_library && matches!(bytes, b"core" | b"alloc" | b"std") {
            return true;
        }
        after_library = bytes == b"library";
    }
    false
}

fn rust_path_module_matches(path: &std::path::Path, owner: &[u8]) -> bool {
    let direct = path
        .file_stem()
        .map(std::ffi::OsStr::as_encoded_bytes)
        .is_some_and(|name| name == owner);
    direct
        || (path.file_stem() == Some(std::ffi::OsStr::new("mod"))
            && path
                .parent()
                .and_then(std::path::Path::file_name)
                .map(std::ffi::OsStr::as_encoded_bytes)
                .is_some_and(|name| name == owner))
        || path
            .components()
            .any(|component| component.as_os_str().as_encoded_bytes() == owner)
}

fn editor_goto_module_file(editor: &mut Editor, name: &[u8]) -> bool {
    profiling::function_scope!();
    let Ok(name) = std::str::from_utf8(name) else {
        return false;
    };
    let Some(parent) = editor_document(editor)
        .path
        .as_deref()
        .and_then(std::path::Path::parent)
    else {
        return false;
    };
    let direct = parent.join(format!("{name}.rs"));
    let nested = parent.join(name).join("mod.rs");
    let path = if direct.is_file() {
        direct
    } else if nested.is_file() {
        nested
    } else {
        return false;
    };
    editor_navigate_to_module(editor, &path);
    true
}

fn editor_goto_workspace_symbol(editor: &mut Editor, owner: &[u8], name: &[u8]) -> bool {
    profiling::function_scope!();
    let preferred_project_file = editor_document(editor).path.as_ref().and_then(|path| {
        editor
            .project
            .paths
            .iter()
            .position(|candidate| candidate == path)
    });
    let Some(corpus) = editor.project_search.as_ref() else {
        return false;
    };
    let Some(symbol) = search_corpus_symbol(
        corpus,
        &editor.project.paths,
        owner,
        name,
        preferred_project_file,
    ) else {
        return false;
    };
    let symbol = corpus.symbols[symbol];
    let project_file = symbol.project_file as usize;
    let Some(path) = editor.project.paths.get(project_file).cloned() else {
        return false;
    };
    editor_navigate_to_symbol(editor, &path, symbol.start as usize, symbol.end as usize);
    true
}

fn search_corpus_symbol(
    corpus: &SearchCorpus,
    project_paths: &[std::path::PathBuf],
    owner: &[u8],
    name: &[u8],
    preferred_project_file: Option<usize>,
) -> Option<usize> {
    profiling::function_scope!();
    let first = corpus.symbols.partition_point(|symbol| {
        &corpus.bytes[symbol.name_start as usize..symbol.name_end as usize] < name
    });
    let last = corpus.symbols.partition_point(|symbol| {
        &corpus.bytes[symbol.name_start as usize..symbol.name_end as usize] <= name
    });
    let candidates = &corpus.symbols[first..last];
    candidates
        .iter()
        .position(|symbol| {
            preferred_project_file == Some(symbol.project_file as usize)
                && (owner.is_empty()
                    || project_paths
                        .get(symbol.project_file as usize)
                        .is_some_and(|path| rust_path_module_matches(path, owner)))
        })
        .or_else(|| {
            candidates.iter().position(|symbol| {
                owner.is_empty()
                    || project_paths
                        .get(symbol.project_file as usize)
                        .is_some_and(|path| rust_path_module_matches(path, owner))
            })
        })
        .map(|position| first + position)
}

fn editor_select_references(editor: &mut Editor) {
    profiling::function_scope!();
    if !editor_document(editor).code_index.enabled {
        editor
            .status
            .push_str("reference provider unavailable for this language");
        return;
    }
    editor_finish_workspace_refresh(editor);
    let temp = idno_std::mem().scratch().temp();
    let mut name = temp.vec(64);
    let mut owner = temp.vec(64);
    let mut glob_owners = temp.vec(64);
    let mut definition = ReferenceDefinition {
        identifier: None,
        position: None,
        rust_symbol: None,
        workspace_symbol: None,
        workspace: true,
    };
    let mut field_owner = temp.vec(64);
    {
        let document = editor_document_mut(editor);
        while !document.code_index.complete {
            code_index_step(
                &document.buffer,
                &mut document.code_index,
                256 * 1024,
                std::time::Duration::MAX,
            );
        }
        let Some(identifier_index) =
            code_index_identifier_at(&document.code_index, document.cursor)
        else {
            editor.status.push_str("no indexed identifier");
            return;
        };
        let identifier = document.code_index.identifiers[identifier_index];
        if let Some(local_definition) =
            code_index_definition_for(&document.buffer, &document.code_index, identifier_index)
        {
            let symbol = document.code_index.symbols[local_definition];
            definition.workspace = symbol.kind != crate::code_index::CodeSymbolKind::Value;
            definition.identifier = Some(symbol.identifier);
            let definition_identifier = document.code_index.identifiers[symbol.identifier as usize];
            definition.position = Some(definition_identifier.start as usize);
            if symbol.kind == CodeSymbolKind::Value {
                rust_struct_field_owner(
                    &document.buffer,
                    definition_identifier.start as usize,
                    &mut field_owner,
                );
                definition.workspace |= !field_owner.is_empty();
            }
        }
        rust_qualified_owner(&document.buffer, identifier.start as usize, &mut owner);
        if owner.is_empty() {
            rust_import_owner(
                &document.buffer,
                &document.code_index,
                identifier_index,
                &mut owner,
            );
        }
        rust_glob_import_owners(
            &document.buffer,
            identifier.start as usize,
            &mut glob_owners,
        );
        for position in identifier.start as usize..identifier.end as usize {
            name.push(buffer_byte(&document.buffer, position));
        }
    }
    if rust_method_index_finish(&mut editor.rust_methods) {
        editor_refresh_completion(editor);
    }
    editor_finish_project_search(editor);
    if definition.identifier.is_none() {
        definition.rust_symbol =
            rust_indexed_symbol(&editor.rust_methods.corpus, &owner, &glob_owners, &name);
    }
    if definition.workspace
        && field_owner.is_empty()
        && let Some(corpus) = editor.project_search.as_ref()
    {
        let preferred_project_file = editor_document(editor).path.as_ref().and_then(|path| {
            editor
                .project
                .paths
                .iter()
                .position(|candidate| candidate == path)
        });
        definition.workspace_symbol = search_corpus_symbol(
            corpus,
            &editor.project.paths,
            &owner,
            &name,
            preferred_project_file,
        );
    }
    editor_open_picker(editor, PickerKind::References);
    let mut picker = editor.picker.take().unwrap();
    search_corpus_index_document(editor_document(editor), &mut picker.preview_corpus);
    reference_targets_collect(editor, &name, definition, &field_owner, &mut picker);
    if picker.reference_targets.is_empty() {
        editor.status.push_str("no references indexed");
        return;
    }
    picker_refresh(editor, &mut picker);
    picker_rebuild_preview(editor, &mut picker);
    editor.picker = Some(picker);
}

fn reference_targets_collect(
    editor: &Editor,
    name: &[u8],
    definition: ReferenceDefinition,
    field_owner: &[u8],
    picker: &mut Picker,
) {
    profiling::function_scope!();
    let temp = idno_std::mem().scratch().temp();
    let mut inferred_owner = temp.vec(64);
    let document = editor_document(editor);
    let current_label = document
        .path
        .as_deref()
        .map(|path| path.strip_prefix(&editor.project.root).unwrap_or(path));
    let current_project_file = document.path.as_ref().and_then(|path| {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        editor.project.paths.iter().position(|candidate| {
            std::fs::canonicalize(candidate).unwrap_or_else(|_| candidate.clone()) == path
        })
    });
    let canonical_workspace_location = definition
        .workspace_symbol
        .and_then(|symbol| {
            editor.project_search.as_ref().and_then(|corpus| {
                corpus
                    .symbols
                    .get(symbol)
                    .map(|symbol| (symbol.project_file as usize, symbol.start as usize))
            })
        })
        .or_else(|| {
            definition.rust_symbol.and_then(|symbol| {
                let Some(rust_symbol) = editor.rust_methods.corpus.symbols.get(symbol) else {
                    return None;
                };
                let Some(path) = rust_symbol_path(&editor.rust_methods.corpus, symbol) else {
                    return None;
                };
                editor
                    .project
                    .paths
                    .iter()
                    .position(|candidate| candidate == path)
                    .map(|project_file| (project_file, rust_symbol.position as usize))
            })
        })
        .or_else(|| current_project_file.zip(definition.position));
    let length = buffer_len(&document.buffer);
    let mut line_start = 0usize;
    let mut line_number = 1;
    let mut line_end = buffer_line_end(&document.buffer, line_start);
    for (indexed_identifier, identifier) in document.code_index.identifiers.iter().enumerate() {
        let position = identifier.start as usize;
        if identifier.end as usize - position != name.len()
            || !(0..name.len())
                .all(|offset| buffer_byte(&document.buffer, position + offset) == name[offset])
        {
            continue;
        }
        while position > line_end && line_end < length {
            line_start = line_end + 1;
            line_end = buffer_line_end(&document.buffer, line_start);
            line_number += 1;
        }
        let same_definition = definition.identifier.is_none_or(|expected| {
            code_index_definition_for(&document.buffer, &document.code_index, indexed_identifier)
                .is_some_and(|symbol| document.code_index.symbols[symbol].identifier == expected)
        });
        if same_definition
            && (field_owner.is_empty()
                || definition.position == Some(position)
                || rust_field_reference_matches_buffer(
                    &document.buffer,
                    position,
                    field_owner,
                    &mut inferred_owner,
                ))
        {
            let label_start = picker.symbol_corpus.bytes.len();
            match current_label {
                Some(path) => {
                    write!(
                        &mut picker.symbol_corpus.bytes,
                        "{}:{line_number}",
                        path.display()
                    )
                    .unwrap();
                }
                None => write!(&mut picker.symbol_corpus.bytes, "[scratch]:{line_number}").unwrap(),
            }
            reference_target_push(
                picker,
                u32::MAX,
                position,
                position + name.len(),
                label_start,
            );
        }
    }
    if !definition.workspace {
        return;
    }
    let Some(corpus) = editor.project_search.as_ref() else {
        return;
    };
    let first = corpus.identifiers.partition_point(|identifier| {
        &corpus.bytes[identifier.name_start as usize..identifier.name_end as usize] < name
    });
    let last = corpus.identifiers.partition_point(|identifier| {
        &corpus.bytes[identifier.name_start as usize..identifier.name_end as usize] <= name
    });
    for identifier in &corpus.identifiers[first..last] {
        let project_file = identifier.project_file as usize;
        if current_project_file == Some(project_file) {
            continue;
        }
        let Some(line) = corpus.lines.get(identifier.line as usize) else {
            continue;
        };
        let source = &corpus.bytes[line.text_start as usize..line.display_end as usize];
        let position = identifier.file_start.saturating_sub(line.file_offset) as usize;
        let declaration = rust_symbol_name_range(source)
            .is_some_and(|range| range.0 == position && range.1 == position + name.len());
        if declaration
            && canonical_workspace_location != Some((project_file, identifier.file_start as usize))
        {
            continue;
        }
        if !field_owner.is_empty()
            && !search_corpus_field_reference_candidate(
                corpus,
                identifier.project_file,
                source,
                position,
                field_owner,
            )
        {
            continue;
        }
        let label_start = picker.symbol_corpus.bytes.len();
        let label_end = (line.text_start as usize)
            .saturating_sub(2)
            .max(line.display_start as usize);
        picker
            .symbol_corpus
            .bytes
            .extend_from_slice(&corpus.bytes[line.display_start as usize..label_end]);
        reference_target_push(
            picker,
            identifier.project_file,
            identifier.file_start as usize,
            identifier.file_end as usize,
            label_start,
        );
    }
}

fn rust_field_reference_matches_buffer(
    buffer: &GapBuffer,
    field: usize,
    owner: &[u8],
    inferred_owner: &mut Vec<u8, impl Allocator>,
) -> bool {
    let mut dot = field;
    while dot > 0 && buffer_byte(buffer, dot - 1).is_ascii_whitespace() {
        dot -= 1;
    }
    if dot == 0 || buffer_byte(buffer, dot - 1) != b'.' {
        return false;
    }
    let mut receiver_end = dot - 1;
    while receiver_end > 0 && buffer_byte(buffer, receiver_end - 1).is_ascii_whitespace() {
        receiver_end -= 1;
    }
    let mut receiver_start = receiver_end;
    while receiver_start > 0 && rust_identifier_byte(buffer_byte(buffer, receiver_start - 1)) {
        receiver_start -= 1;
    }
    if receiver_start == receiver_end {
        return false;
    }
    (rust_explicit_type(buffer, receiver_start, receiver_end, inferred_owner)
        || rust_closure_parameter_element_type(
            buffer,
            receiver_start,
            receiver_end,
            inferred_owner,
        ))
        && inferred_owner == owner
}

fn rust_closure_parameter_element_type(
    buffer: &GapBuffer,
    receiver_start: usize,
    receiver_end: usize,
    owner: &mut Vec<u8, impl Allocator>,
) -> bool {
    profiling::function_scope!();
    let mut close_pipe = receiver_start;
    while close_pipe > 0 && buffer_byte(buffer, close_pipe - 1) != b'|' {
        close_pipe -= 1;
    }
    if close_pipe == 0 {
        return false;
    }
    close_pipe -= 1;
    let mut open_pipe = close_pipe;
    while open_pipe > 0 && buffer_byte(buffer, open_pipe - 1) != b'|' {
        open_pipe -= 1;
    }
    if open_pipe == 0 {
        return false;
    }
    open_pipe -= 1;
    let parameter_present = (open_pipe + 1..close_pipe).any(|position| {
        position + receiver_end - receiver_start <= close_pipe
            && (0..receiver_end - receiver_start).all(|offset| {
                buffer_byte(buffer, position + offset)
                    == buffer_byte(buffer, receiver_start + offset)
            })
            && (position == open_pipe + 1
                || !rust_identifier_byte(buffer_byte(buffer, position - 1)))
            && (position + receiver_end - receiver_start == close_pipe
                || !rust_identifier_byte(buffer_byte(
                    buffer,
                    position + receiver_end - receiver_start,
                )))
    });
    if !parameter_present {
        return false;
    }

    let mut position = open_pipe;
    while position > 0 && buffer_byte(buffer, position - 1).is_ascii_whitespace() {
        position -= 1;
    }
    if position == 0 || buffer_byte(buffer, position - 1) != b'(' {
        return false;
    }
    position -= 1;
    while position > 0 && buffer_byte(buffer, position - 1).is_ascii_whitespace() {
        position -= 1;
    }
    while position > 0 && rust_identifier_byte(buffer_byte(buffer, position - 1)) {
        position -= 1;
    }
    if position == 0 || buffer_byte(buffer, position - 1) != b'.' {
        return false;
    }
    position -= 1;
    while position > 0 && buffer_byte(buffer, position - 1).is_ascii_whitespace() {
        position -= 1;
    }
    let collection_end = position;
    while position > 0 && rust_identifier_byte(buffer_byte(buffer, position - 1)) {
        position -= 1;
    }
    if position == collection_end {
        return false;
    }
    rust_named_collection_element_type(buffer, position, collection_end, owner)
}

fn rust_named_collection_element_type(
    buffer: &GapBuffer,
    name_start: usize,
    name_end: usize,
    owner: &mut Vec<u8, impl Allocator>,
) -> bool {
    profiling::function_scope!();
    let length = buffer_len(buffer);
    let name_length = name_end - name_start;
    let mut candidate = 0usize;
    while candidate + name_length <= length {
        let equal = (0..name_length).all(|offset| {
            buffer_byte(buffer, candidate + offset) == buffer_byte(buffer, name_start + offset)
        });
        let before = candidate == 0 || !rust_identifier_byte(buffer_byte(buffer, candidate - 1));
        let after = candidate + name_length;
        let after_boundary = after == length || !rust_identifier_byte(buffer_byte(buffer, after));
        if !equal || !before || !after_boundary {
            candidate += 1;
            continue;
        }
        let mut position = after;
        while position < length && buffer_byte(buffer, position).is_ascii_whitespace() {
            position += 1;
        }
        if position >= length || buffer_byte(buffer, position) != b':' {
            candidate = after;
            continue;
        }
        position += 1;
        let limit = (position + 256).min(length);
        while position + 3 <= limit {
            if buffer_range_matches_identifier(buffer, position, limit, b"Vec") {
                position += 3;
                while position < limit && buffer_byte(buffer, position).is_ascii_whitespace() {
                    position += 1;
                }
                if position >= limit || buffer_byte(buffer, position) != b'<' {
                    break;
                }
                position += 1;
                while position < limit
                    && matches!(buffer_byte(buffer, position), b'&' | b' ' | b'\t')
                {
                    position += 1;
                }
                owner.clear();
                while position < limit {
                    let start = position;
                    while position < limit && rust_identifier_byte(buffer_byte(buffer, position)) {
                        position += 1;
                    }
                    if start == position {
                        return !owner.is_empty();
                    }
                    owner.clear();
                    for byte_position in start..position {
                        owner.push(buffer_byte(buffer, byte_position));
                    }
                    if position + 1 < limit
                        && buffer_byte(buffer, position) == b':'
                        && buffer_byte(buffer, position + 1) == b':'
                    {
                        position += 2;
                        continue;
                    }
                    return true;
                }
            }
            if matches!(buffer_byte(buffer, position), b';' | b'=' | b'\n') {
                break;
            }
            position += 1;
        }
        candidate = after;
    }
    false
}

fn search_corpus_field_reference_candidate(
    corpus: &SearchCorpus,
    project_file: u32,
    source: &[u8],
    position: usize,
    owner: &[u8],
) -> bool {
    let mut before = position;
    while before > 0 && source[before - 1].is_ascii_whitespace() {
        before -= 1;
    }
    if before == 0 || source[before - 1] != b'.' {
        return source[..position]
            .windows(owner.len())
            .any(|candidate| candidate == owner);
    }
    let mut receiver_end = before - 1;
    while receiver_end > 0 && source[receiver_end - 1].is_ascii_whitespace() {
        receiver_end -= 1;
    }
    let mut receiver_start = receiver_end;
    while receiver_start > 0 && rust_identifier_byte(source[receiver_start - 1]) {
        receiver_start -= 1;
    }
    if receiver_start == receiver_end {
        return false;
    }
    let receiver = &source[receiver_start..receiver_end];
    let first = corpus
        .lines
        .partition_point(|line| line.project_file < project_file);
    let last = corpus
        .lines
        .partition_point(|line| line.project_file <= project_file);
    for line in &corpus.lines[first..last] {
        let candidate = &corpus.bytes[line.text_start as usize..line.display_end as usize];
        let receiver_position = slice_identifier_position(candidate, receiver);
        let owner_position = slice_identifier_position(candidate, owner);
        if receiver_position.is_some_and(|position| {
            owner_position.is_some_and(|owner_position| {
                owner_position > position
                    && candidate[position + receiver.len()..owner_position]
                        .iter()
                        .any(|byte| matches!(byte, b':' | b'='))
            })
        }) {
            return true;
        }
        if receiver == b"self"
            && slice_identifier_position(candidate, b"impl").is_some_and(|position| {
                owner_position.is_some_and(|owner_position| owner_position > position)
            })
        {
            return true;
        }
    }
    false
}

fn slice_identifier_position(source: &[u8], name: &[u8]) -> Option<usize> {
    source
        .windows(name.len())
        .enumerate()
        .find_map(|(position, candidate)| {
            let end = position + name.len();
            (candidate == name
                && (position == 0 || !rust_identifier_byte(source[position - 1]))
                && (end == source.len() || !rust_identifier_byte(source[end])))
            .then_some(position)
        })
}

fn rust_struct_field_owner(buffer: &GapBuffer, field: usize, owner: &mut Vec<u8, impl Allocator>) {
    profiling::function_scope!();
    owner.clear();
    let mut position = field;
    let mut depth = 0usize;
    let body_start = loop {
        if position == 0 {
            return;
        }
        position -= 1;
        match buffer_byte(buffer, position) {
            b'}' => depth += 1,
            b'{' if depth == 0 => break position,
            b'{' => depth = depth.saturating_sub(1),
            _ => {}
        }
    };
    let search_start = body_start.saturating_sub(512);
    let mut struct_position = None;
    let mut candidate = search_start;
    while candidate + 6 <= body_start {
        if buffer_range_matches_identifier(buffer, candidate, body_start, b"struct") {
            struct_position = Some(candidate + 6);
        }
        candidate += 1;
    }
    let Some(mut position) = struct_position else {
        return;
    };
    while position < body_start && buffer_byte(buffer, position).is_ascii_whitespace() {
        position += 1;
    }
    while position < body_start && rust_identifier_byte(buffer_byte(buffer, position)) {
        owner.push(buffer_byte(buffer, position));
        position += 1;
    }
}

#[cfg(test)]
fn rust_line_position_is_code(source: &[u8], target: usize) -> bool {
    let mut position = 0;
    let mut delimiter = 0;
    let mut escaped = false;
    while position < target.min(source.len()) {
        let byte = source[position];
        let next = source.get(position + 1).copied();
        if delimiter != 0 {
            if byte == b'\\' && !escaped {
                escaped = true;
            } else {
                if byte == delimiter && !escaped {
                    delimiter = 0;
                }
                escaped = false;
            }
            position += 1;
        } else if byte == b'/' && next == Some(b'/') {
            return false;
        } else if byte == b'/' && next == Some(b'*') {
            let Some(end) = source[position + 2..]
                .windows(2)
                .position(|window| window == b"*/")
            else {
                return false;
            };
            position += end + 4;
        } else if byte == b'"' || (byte == b'\'' && rust_line_char_literal(source, position)) {
            delimiter = byte;
            escaped = false;
            position += 1;
        } else {
            position += 1;
        }
    }
    delimiter == 0
}

fn rust_source_identifiers(
    source: &[u8],
    identifiers: &mut Vec<std::ops::Range<u32>, impl Allocator>,
) {
    profiling::function_scope!();
    identifiers.clear();
    if source.len() > u32::MAX as usize {
        return;
    }
    let mut position = 0usize;
    let mut block_depth = 0usize;
    let mut delimiter = 0u8;
    let mut escaped = false;
    while position < source.len() {
        let byte = source[position];
        let next = source.get(position + 1).copied();
        if block_depth > 0 {
            if byte == b'/' && next == Some(b'*') {
                block_depth += 1;
                position += 2;
            } else if byte == b'*' && next == Some(b'/') {
                block_depth -= 1;
                position += 2;
            } else {
                position += 1;
            }
        } else if delimiter != 0 {
            if byte == b'\\' && !escaped {
                escaped = true;
            } else {
                if byte == delimiter && !escaped {
                    delimiter = 0;
                }
                escaped = false;
            }
            position += 1;
        } else if byte == b'/' && next == Some(b'/') {
            position += 2;
            while position < source.len() && source[position] != b'\n' {
                position += 1;
            }
        } else if byte == b'/' && next == Some(b'*') {
            block_depth = 1;
            position += 2;
        } else if byte == b'"' || (byte == b'\'' && rust_line_char_literal(source, position)) {
            delimiter = byte;
            escaped = false;
            position += 1;
        } else if rust_identifier_byte(byte) && !byte.is_ascii_digit() {
            let start = position;
            position += 1;
            while position < source.len() && rust_identifier_byte(source[position]) {
                position += 1;
            }
            identifiers.push(start as u32..position as u32);
        } else {
            position += 1;
        }
    }
}

fn rust_line_char_literal(source: &[u8], position: usize) -> bool {
    let mut end = position + 1;
    if source.get(end) == Some(&b'\\') {
        end += 2;
    } else {
        end += 1;
        while end < source.len() && source[end] & 0b1100_0000 == 0b1000_0000 {
            end += 1;
        }
    }
    source.get(end) == Some(&b'\'')
}

fn reference_target_push(
    picker: &mut Picker,
    project_file: u32,
    start: usize,
    end: usize,
    label_start: usize,
) {
    if end > u32::MAX as usize || picker.symbol_corpus.bytes.len() > u32::MAX as usize {
        picker.symbol_corpus.bytes.truncate(label_start);
        return;
    }
    let label_end = picker.symbol_corpus.bytes.len();
    picker.symbol_corpus.lines.push(SearchLine {
        project_file,
        file_offset: start as u32,
        line_number: 0,
        text_start: label_start as u32,
        display_start: label_start as u32,
        display_end: label_end as u32,
    });
    picker.reference_targets.push(ReferenceTarget {
        project_file,
        start: start as u32,
        end: end as u32,
    });
}

fn buffer_range_matches_identifier(
    buffer: &GapBuffer,
    start: usize,
    line_end: usize,
    name: &[u8],
) -> bool {
    let before = start == 0 || !rust_identifier_byte(buffer_byte(buffer, start - 1));
    let end = start + name.len();
    before
        && (end >= line_end || !rust_identifier_byte(buffer_byte(buffer, end)))
        && name
            .iter()
            .enumerate()
            .all(|(offset, &byte)| buffer_byte(buffer, start + offset) == byte)
}

#[inline]
fn rust_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn editor_goto_last_line(editor: &mut Editor) {
    let document = editor_document(editor);
    let length = buffer_len(&document.buffer);
    let position = if length > 0 && buffer_byte(&document.buffer, length - 1) == b'\n' {
        buffer_line_start(&document.buffer, length - 1)
    } else {
        buffer_line_start(&document.buffer, length)
    };
    editor_goto_absolute_position(editor, position);
}

fn editor_goto_absolute_position(editor: &mut Editor, position: usize) {
    let before = editor_location(editor);
    let mode = editor.mode;
    let document = editor_document_mut(editor);
    let position = position.min(buffer_len(&document.buffer));
    if mode == Mode::Select {
        let temp = idno_std::mem().scratch().temp();
        let mut selections = temp.vec(document.secondary_selections.len() + 1);
        document_selections(document, &mut selections);
        for selection in &mut selections {
            selection.cursor = position;
        }
        document_set_selections(document, &selections);
    } else {
        document.secondary_selections.clear();
        document.cursor = position;
        document.anchor = position;
    }
    document.preferred_column = buffer_line_and_column(&document.buffer, position).1;
    let after = editor_location(editor);
    editor_record_jump(editor, before, after);
}

fn editor_select_symbol(editor: &mut Editor, start: usize, end: usize) {
    profiling::function_scope!();
    let before = editor_location(editor);
    editor_set_symbol_selection(editor, start, end);
    editor_center_view(editor);
    let after = editor_location(editor);
    editor_record_jump(editor, before, after);
}

fn editor_set_symbol_selection(editor: &mut Editor, start: usize, end: usize) {
    let document = editor_document_mut(editor);
    let start = start.min(buffer_len(&document.buffer));
    let end = end.min(buffer_len(&document.buffer));
    document.cursor = start;
    document.anchor = if end > start {
        buffer_previous_char(&document.buffer, end)
    } else {
        start
    };
    document.secondary_selections.clear();
    document.preferred_column = buffer_line_and_column(&document.buffer, start).1;
    editor.mode = Mode::Normal;
}

fn editor_navigate_to_symbol(
    editor: &mut Editor,
    path: &std::path::Path,
    start: usize,
    end: usize,
) {
    profiling::function_scope!();
    let before = editor_location(editor);
    let Some(target) = editor_document_target(editor, path.to_path_buf()) else {
        return;
    };
    editor_switch_document_state(editor, target);
    editor_set_symbol_selection(editor, start, end);
    editor_center_view(editor);
    let after = editor_location(editor);
    editor_record_jump(editor, before, after);
}

fn editor_navigate_to_module(editor: &mut Editor, path: &std::path::Path) {
    profiling::function_scope!();
    let before = editor_location(editor);
    let Some(target) = editor_document_target(editor, path.to_path_buf()) else {
        return;
    };
    editor_switch_document_state(editor, target);
    let length = buffer_len(&editor_document(editor).buffer);
    editor_set_symbol_selection(editor, 0, length);
    editor_center_view(editor);
    let after = editor_location(editor);
    editor_record_jump(editor, before, after);
}

fn editor_center_view(editor: &mut Editor) {
    profiling::function_scope!();
    let visible_lines = editor.viewport_height.max(1);
    let document = editor_document_mut(editor);
    let cursor_line = buffer_line_and_column(&document.buffer, document.cursor).0;
    document.top_line = cursor_line.saturating_sub(visible_lines / 2);
}

fn editor_goto_first_nonwhitespace(editor: &mut Editor) {
    let mode = editor.mode;
    let document = editor_document_mut(editor);
    let temp = idno_std::mem().scratch().temp();
    let mut selections = temp.vec(document.secondary_selections.len() + 1);
    document_selections(document, &mut selections);
    for selection in &mut selections {
        let mut position = buffer_line_start(&document.buffer, selection.cursor);
        let line_end = buffer_line_end(&document.buffer, position);
        while position < line_end && matches!(buffer_byte(&document.buffer, position), b' ' | b'\t')
        {
            position += 1;
        }
        selection.cursor = position;
        if mode == Mode::Normal {
            selection.anchor = position;
        }
    }
    document_set_selections(document, &selections);
}

fn editor_goto_last_accessed_document(editor: &mut Editor) {
    let Some(target) = editor.last_accessed_document else {
        return;
    };
    editor_switch_document(editor, target);
}

fn editor_goto_window_line(editor: &mut Editor, alignment: usize) {
    let document = editor_document(editor);
    let margin = editor
        .config
        .scroll_margin_lines
        .min(editor.viewport_height.saturating_sub(1) / 2);
    let visible_lines = editor.viewport_height.max(1);
    let target_line = match alignment {
        0 => document.top_line + margin,
        1 => document.top_line + visible_lines / 2,
        _ => document.top_line + visible_lines.saturating_sub(margin + 1),
    };
    let position = buffer_position_at_line_column(&document.buffer, target_line, 0);
    editor_goto_absolute_position(editor, position);
}

fn editor_move_line_boundary(editor: &mut Editor, end: bool) {
    let mode = editor.mode;
    let document = editor_document_mut(editor);
    let temp = idno_std::mem().scratch().temp();
    let mut selections = temp.vec(document.secondary_selections.len() + 1);
    document_selections(document, &mut selections);
    for selection in &mut selections {
        selection.cursor = if end {
            let line_end = buffer_line_end(&document.buffer, selection.cursor);
            if mode == Mode::Insert {
                line_end
            } else {
                buffer_previous_char(&document.buffer, line_end)
            }
        } else {
            buffer_line_start(&document.buffer, selection.cursor)
        };
        if mode == Mode::Normal {
            selection.anchor = selection.cursor;
        }
    }
    document_set_selections(document, &selections);
    document.preferred_column = buffer_line_and_column(&document.buffer, document.cursor).1;
    if mode == Mode::Normal {
        document.anchor = document.cursor;
    }
}

fn command_cursor_clamped(buffer: &GapBuffer, mut cursor: usize, mode: Mode) -> usize {
    let ends_with_newline = cursor > 0 && buffer_byte(buffer, cursor - 1) == b'\n';
    if mode != Mode::Insert && cursor == buffer_len(buffer) && cursor > 0 && !ends_with_newline {
        cursor = buffer_previous_char(buffer, cursor);
    }
    cursor
}

fn editor_render(editor: &mut Editor, terminal: &mut Terminal) -> std::io::Result<()> {
    let (width, height) = terminal::terminal_size(terminal);
    let picker = editor.picker.is_some();
    let picker_changed = picker != editor.rendered_picker;
    let background_changed = width != editor.terminal_width
        || height != editor.terminal_height
        || editor.theme != editor.rendered_theme;
    let force = background_changed || picker_changed;
    editor.terminal_width = width;
    editor.terminal_height = height;
    editor.rendered_picker = picker;
    editor.rendered_theme = editor.theme;
    if picker {
        if picker_changed || background_changed {
            editor.rendered_row_hashes.clear();
            editor.rendered_overlay_start = None;
            if let Err(error) = editor_render_document(editor, terminal, width, height, true) {
                return Err(error);
            }
        }
        return editor_render_picker(editor, terminal, width, height);
    }
    editor_render_document(editor, terminal, width, height, force)
}

fn editor_render_document(
    editor: &mut Editor,
    terminal: &mut Terminal,
    width: usize,
    height: usize,
    force: bool,
) -> std::io::Result<()> {
    profiling::function_scope!();
    let theme = &THEMES[editor.theme];
    let content_height = height.saturating_sub(1);
    editor.viewport_height = content_height;
    let mode = editor.mode;
    let scroll_margin = editor
        .config
        .scroll_margin_lines
        .min(content_height.saturating_sub(1) / 2);
    let current = editor.current;
    let project_root = &editor.project.root;
    let search = editor
        .search
        .as_ref()
        .filter(|search| search.document == current);
    let active_search_selection = search
        .and_then(|search| search.matches.get(search.selected))
        .copied();
    let completion_preview = editor.completion.as_ref().and_then(|completion| {
        if !completion.preview {
            return None;
        }
        completion.matches.get(completion.selected).map(|&entry| {
            (
                entry,
                &completion.bytes[entry.insertion_start as usize..entry.insertion_end as usize],
            )
        })
    });
    let temp = idno_std::mem().scratch().temp();
    let document_path = editor.documents[current].path.as_deref();
    let total_document_lines = buffer_line_count(&editor.documents[current].buffer);
    let diagnostic_cursor_line = buffer_line_and_column(
        &editor.documents[current].buffer,
        editor.documents[current].cursor,
    )
    .0;
    let mut diagnostic_severities = temp.vec(total_document_lines);
    diagnostic_severities.extend(std::iter::repeat_n(u8::MAX, total_document_lines));
    let cursor_diagnostic = editor
        .diagnostics
        .published
        .iter()
        .enumerate()
        .filter(|(_, diagnostic)| Some(diagnostic.path.as_path()) == document_path)
        .filter(|(_, diagnostic)| {
            diagnostic.line as usize <= diagnostic_cursor_line
                && diagnostic_cursor_line <= diagnostic.end_line as usize
        })
        .min_by_key(|(_, diagnostic)| diagnostic.severity)
        .map(|(index, _)| index);
    for diagnostic in editor
        .diagnostics
        .published
        .iter()
        .filter(|diagnostic| Some(diagnostic.path.as_path()) == document_path)
    {
        let first = (diagnostic.line as usize).min(diagnostic_severities.len());
        let end = (diagnostic.end_line as usize + 1).min(diagnostic_severities.len());
        for severity in &mut diagnostic_severities[first..end] {
            *severity = (*severity).min(diagnostic.severity as u8);
        }
    }
    let document = &mut editor.documents[current];
    let primary_position = if let Some(selection) = active_search_selection {
        selection.cursor
    } else if mode == Mode::Insert {
        document
            .insertion_points
            .last()
            .copied()
            .unwrap_or(document.cursor)
    } else {
        document.cursor
    };
    let tab_width = editor.config.indentation_spaces.max(1);
    let cursor_line = buffer_line_and_column(&document.buffer, primary_position).0;
    let cursor_column = buffer_terminal_column(&document.buffer, primary_position, tab_width);
    let preview_cursor_column = completion_preview.map_or(cursor_column, |(entry, insertion)| {
        let insertion_cursor = if entry.selection_end > entry.selection_start {
            entry.selection_start as usize
        } else {
            insertion.len()
        }
        .min(insertion.len());
        let base = buffer_terminal_column(&document.buffer, entry.replacement_start, tab_width);
        let preview_width = std::str::from_utf8(&insertion[..insertion_cursor])
            .map_or(insertion_cursor, |text| {
                terminal_text_width(text, usize::MAX)
            });
        base + preview_width
    });
    if cursor_line < document.top_line + scroll_margin {
        document.top_line = cursor_line.saturating_sub(scroll_margin);
    } else if cursor_line + scroll_margin >= document.top_line + content_height {
        document.top_line = cursor_line + scroll_margin + 1 - content_height;
    }
    let top_line = document.top_line;
    let total_lines = buffer_line_count(&document.buffer);
    let number_width = (total_lines.max(1).ilog10() as usize + 1).max(5);
    let gutter_width = (number_width + 2).min(width);
    let text_width = width.saturating_sub(gutter_width);
    let mut selections = temp.vec(document.secondary_selections.len() + 1);
    let mut row_ranges = temp.vec(content_height);
    document_selections(document, &mut selections);
    let path = document
        .path
        .as_deref()
        .map(|path| path.strip_prefix(project_root).unwrap_or(path));
    let dirty = document.modified;

    editor.frame.clear();
    editor
        .frame
        .extend_from_slice(b"\x1b[?2026h\x1b[?7l\x1b[?25l\x1b[H");
    editor.frame.extend_from_slice(theme.cursor_color);
    editor.frame.extend_from_slice(theme.normal);
    let mut position = buffer_position_at_line_column(&document.buffer, top_line, 0);
    let syntax_spans = syntax_highlighting_spans(&document.syntax);
    let mut syntax_span_position =
        syntax_spans.partition_point(|span| span.end as usize <= position);
    let length = buffer_len(&document.buffer);
    let end_line = buffer_line_and_column(&document.buffer, length).0;
    let trailing_empty_line = length > 0 && buffer_byte(&document.buffer, length - 1) == b'\n';
    let secondary_insertion_points = if mode == Mode::Insert {
        &document.insertion_points[..document.insertion_points.len().saturating_sub(1)]
    } else {
        &[]
    };
    let mut rendered_style = 0u8;
    for row in 0..content_height {
        let row_start = editor.frame.len();
        let line = top_line + row;
        editor.frame.extend_from_slice(theme.gutter);
        let diagnostic_severity = diagnostic_severities.get(line).copied().unwrap_or(u8::MAX);
        if diagnostic_severity != u8::MAX {
            let severity = match diagnostic_severity {
                value if value == DiagnosticSeverity::Error as u8 => DiagnosticSeverity::Error,
                value if value == DiagnosticSeverity::Warning as u8 => DiagnosticSeverity::Warning,
                _ => DiagnosticSeverity::Info,
            };
            editor
                .frame
                .extend_from_slice(diagnostic_severity_style(severity, theme));
            editor.frame.extend_from_slice("●".as_bytes());
            editor.frame.extend_from_slice(theme.gutter);
        } else {
            editor.frame.push(b' ');
        }
        if line < total_lines && !(trailing_empty_line && line + 1 == total_lines) {
            write!(&mut editor.frame, "{:>number_width$}", line + 1).unwrap();
        } else {
            write!(&mut editor.frame, "{:>number_width$}", "~").unwrap();
        }
        let git_flags = git_gutter_flags(&document.git_gutter, line);
        if git_gutter_line_removed(git_flags) {
            editor.frame.extend_from_slice(theme.git_removed);
            editor.frame.extend_from_slice("▔".as_bytes());
        } else if git_gutter_line_modified(git_flags) {
            editor.frame.extend_from_slice(theme.git_modified);
            editor.frame.extend_from_slice("▎".as_bytes());
        } else if git_gutter_line_added(git_flags) {
            editor.frame.extend_from_slice(theme.git_added);
            editor.frame.extend_from_slice("▎".as_bytes());
        } else {
            editor.frame.push(b' ');
        }
        editor.frame.extend_from_slice(theme.normal);
        let mut column = 0;
        while position < length
            && buffer_byte(&document.buffer, position) != b'\n'
            && column < text_width
        {
            if let Some((entry, insertion)) = completion_preview
                && position == entry.replacement_start
                && primary_position >= position
                && buffer_line_start(&document.buffer, primary_position)
                    == buffer_line_start(&document.buffer, position)
            {
                column += append_rust_completion_text(
                    insertion,
                    text_width - column,
                    Some(entry.selection_start as usize..entry.selection_end as usize),
                    theme,
                    false,
                    &mut editor.frame,
                );
                position = primary_position;
                rendered_style = u8::MAX - 1;
                continue;
            }
            while syntax_span_position < syntax_spans.len()
                && syntax_spans[syntax_span_position].end as usize <= position
            {
                syntax_span_position += 1;
            }
            let syntax_style = syntax_spans
                .get(syntax_span_position)
                .filter(|span| span.start as usize <= position && position < span.end as usize)
                .map(|span| 4 + span.kind as u8);
            let secondary_insert_cursor = secondary_insertion_points.contains(&position);
            let normal_cursor = search.is_none()
                && mode != Mode::Insert
                && selections
                    .iter()
                    .any(|selection| selection.cursor == position);
            let position_selected = search.is_none()
                && position_is_visibly_selected(&document.buffer, &selections, position, mode);
            let search_cursor = search.is_some_and(|search| {
                if search.kind == SearchKind::Selection {
                    search
                        .matches
                        .iter()
                        .any(|selection| selection.cursor == position)
                } else {
                    active_search_selection.is_some_and(|selection| selection.cursor == position)
                }
            });
            let position_search_match = search.is_some_and(|search| {
                if search.kind == SearchKind::Selection {
                    position_is_selected(&document.buffer, &search.matches, position)
                } else {
                    active_search_selection.is_some_and(|selection| {
                        position_is_selected(
                            &document.buffer,
                            std::slice::from_ref(&selection),
                            position,
                        )
                    })
                }
            });
            let selected = position_selected || position_search_match;
            let wanted_style = if secondary_insert_cursor || normal_cursor || search_cursor {
                u8::MAX
            } else {
                syntax_style.unwrap_or(0) | if selected { 1 << 7 } else { 0 }
            };
            if wanted_style != rendered_style {
                if wanted_style == u8::MAX {
                    editor.frame.extend_from_slice(theme.cursor);
                } else {
                    let syntax_style = wanted_style & !(1 << 7);
                    editor.frame.extend_from_slice(if syntax_style >= 4 {
                        theme.syntax[(syntax_style - 4) as usize]
                    } else {
                        theme.normal
                    });
                    if wanted_style & (1 << 7) != 0 {
                        editor.frame.extend_from_slice(theme.selection_background);
                    }
                }
                rendered_style = wanted_style;
            }
            let byte = buffer_byte(&document.buffer, position);
            match byte {
                b'\t' => {
                    let spaces = (tab_width - column % tab_width).min(text_width - column);
                    editor.frame.extend(std::iter::repeat_n(b' ', spaces));
                    column += spaces;
                    position += 1;
                }
                0x00..=0x1f | 0x7f => {
                    editor.frame.push(b'?');
                    column += 1;
                    position += 1;
                }
                0x20..=0x7e => {
                    editor.frame.push(byte);
                    column += 1;
                    position += 1;
                }
                _ => {
                    let (next, character_width) = append_buffer_character_within_terminal_width(
                        &document.buffer,
                        position,
                        column,
                        text_width,
                        &mut editor.frame,
                    );
                    if column + character_width > text_width {
                        position = next;
                        break;
                    }
                    column += character_width;
                    position = next;
                }
            }
        }
        if let Some((entry, insertion)) = completion_preview
            && position == entry.replacement_start
            && primary_position >= position
            && buffer_line_start(&document.buffer, primary_position)
                == buffer_line_start(&document.buffer, position)
            && column < text_width
        {
            column += append_rust_completion_text(
                insertion,
                text_width - column,
                Some(entry.selection_start as usize..entry.selection_end as usize),
                theme,
                false,
                &mut editor.frame,
            );
            position = primary_position;
        }
        while position < length && buffer_byte(&document.buffer, position) != b'\n' {
            position += 1;
        }
        if position < length && buffer_byte(&document.buffer, position) == b'\n' {
            let secondary_insert_cursor = secondary_insertion_points.contains(&position);
            let normal_cursor = search.is_none()
                && mode != Mode::Insert
                && selections
                    .iter()
                    .any(|selection| selection.cursor == position);
            let position_selected = search.is_none()
                && position_is_visibly_selected(&document.buffer, &selections, position, mode);
            let search_cursor = search.is_some_and(|search| {
                if search.kind == SearchKind::Selection {
                    search
                        .matches
                        .iter()
                        .any(|selection| selection.cursor == position)
                } else {
                    active_search_selection.is_some_and(|selection| selection.cursor == position)
                }
            });
            let position_search_match = search.is_some_and(|search| {
                if search.kind == SearchKind::Selection {
                    position_is_selected(&document.buffer, &search.matches, position)
                } else {
                    active_search_selection.is_some_and(|selection| {
                        position_is_selected(
                            &document.buffer,
                            std::slice::from_ref(&selection),
                            position,
                        )
                    })
                }
            });
            if column < text_width {
                let style = if secondary_insert_cursor || normal_cursor || search_cursor {
                    theme.cursor
                } else if position_selected || position_search_match {
                    editor.frame.extend_from_slice(theme.normal);
                    theme.selection_background
                } else {
                    theme.normal
                };
                if style != theme.normal {
                    editor.frame.extend_from_slice(style);
                    editor.frame.push(b' ');
                }
            }
            position += 1;
        } else if line == end_line
            && column < text_width
            && ((mode != Mode::Insert
                && selections
                    .iter()
                    .any(|selection| selection.cursor == length))
                || (mode == Mode::Insert && secondary_insertion_points.contains(&length)))
        {
            editor.frame.extend_from_slice(theme.cursor);
            editor.frame.push(b' ');
        }
        editor.frame.extend_from_slice(theme.normal);
        editor.frame.extend_from_slice(b"\x1b[K");
        if row == 0
            && let Some(diagnostic) =
                cursor_diagnostic.and_then(|index| editor.diagnostics.published.get(index))
            && width >= 24
        {
            let message = &diagnostic.display[diagnostic.message_start as usize..];
            let maximum = (width / 2).max(20);
            let message_width = terminal_text_width(message, maximum).min(maximum);
            let column = width.saturating_sub(message_width + 2) + 1;
            write!(&mut editor.frame, "\x1b[1;{column}H").unwrap();
            editor
                .frame
                .extend_from_slice(diagnostic_severity_style(diagnostic.severity, theme));
            editor.frame.push(b' ');
            append_terminal_text(message, maximum, &mut editor.frame);
            editor.frame.push(b' ');
        }
        row_ranges.push(row_start..editor.frame.len());
        rendered_style = 0;
        if row + 1 < content_height {
            editor.frame.extend_from_slice(b"\r\n");
        }
    }
    let status_start = editor.frame.len();
    editor.frame.extend_from_slice(theme.status);
    editor.frame.extend_from_slice(b"\x1b[");
    write!(&mut editor.frame, "{};1H", height).unwrap();
    if let Some(search) = search {
        editor.frame.push(match search.kind {
            SearchKind::Document => b'/',
            SearchKind::Selection => b's',
        });
        write!(
            &mut editor.frame,
            "{}  {}/{}",
            search.query,
            search.selected.saturating_add(1).min(search.matches.len()),
            search.matches.len()
        )
        .unwrap();
    } else {
        write!(&mut editor.frame, " {:?}  ", mode).unwrap();
        match path {
            Some(path) => write!(&mut editor.frame, "{}", path.display()).unwrap(),
            None => editor.frame.extend_from_slice(b"[scratch]"),
        }
        write!(
            &mut editor.frame,
            "{}  {}:{} ",
            if dirty { " [+]" } else { "" },
            cursor_line + 1,
            cursor_column + 1
        )
        .unwrap();
        if editor.pending_key == PendingKey::Goto && editor.pending_count > 0 {
            write!(&mut editor.frame, " g{}", editor.pending_count).unwrap();
        }
        if !editor.status.is_empty() {
            write!(&mut editor.frame, " {}", editor.status).unwrap();
        }
    }
    editor.frame.extend_from_slice(b"\x1b[K");
    editor.frame.extend_from_slice(theme.normal);
    let key_hints = pending_key_hints(editor.pending_key);
    if !key_hints.is_empty() && width > 0 && content_height > 0 {
        let visible_hints = key_hints.len().min(content_height);
        let key_width = key_hints
            .iter()
            .take(visible_hints)
            .map(|hint| hint.key.len())
            .max()
            .unwrap_or(0);
        let popup_width = key_hints
            .iter()
            .take(visible_hints)
            .map(|hint| key_width + 3 + hint.description.len())
            .max()
            .unwrap_or(0)
            .min(width);
        let popup_column = width.saturating_sub(popup_width) + 1;
        let popup_row = content_height.saturating_sub(visible_hints) + 1;
        for (row, hint) in key_hints.iter().take(visible_hints).enumerate() {
            write!(
                &mut editor.frame,
                "\x1b[{};{}H",
                popup_row + row,
                popup_column
            )
            .unwrap();
            editor.frame.extend_from_slice(theme.status);
            write!(
                &mut editor.frame,
                " {:<key_width$}  {:<description_width$}",
                hint.key,
                hint.description,
                description_width = popup_width.saturating_sub(key_width + 3)
            )
            .unwrap();
        }
        editor.frame.extend_from_slice(theme.normal);
    }
    let mut completion_overlay_start = None;
    if key_hints.is_empty()
        && let Some(completion) = editor.completion.as_ref()
        && width > 0
        && content_height > 0
    {
        let visible = completion
            .matches
            .len()
            .min((content_height / 3).clamp(4, 16))
            .min(content_height);
        let first = completion
            .selected
            .saturating_sub(visible.saturating_sub(1));
        let name_width = completion.matches[first..first + visible]
            .iter()
            .map(|&entry| completion_name(completion, entry).len() + 2)
            .max()
            .unwrap_or(0)
            .clamp(12.min(width), (width / 3).max(12).min(width));
        let detail = completion_detail(completion, completion.matches[completion.selected]);
        let detail_width = if width >= 50 && !detail.is_empty() {
            width.saturating_sub(name_width + 1).min(width / 2)
        } else {
            0
        };
        let popup_width = name_width + usize::from(detail_width > 0) + detail_width;
        let scrollbar = completion.matches.len() > visible && name_width >= 3;
        let scrollbar_row = if scrollbar {
            first.saturating_mul(visible.saturating_sub(1))
                / completion.matches.len().saturating_sub(visible).max(1)
        } else {
            0
        };
        let cursor_screen_row = cursor_line.saturating_sub(top_line) + 1;
        let cursor_screen_column = (gutter_width + cursor_column).min(width.saturating_sub(1)) + 1;
        let popup_column = cursor_screen_column.min(width.saturating_sub(popup_width) + 1);
        let rows_below = content_height.saturating_sub(cursor_screen_row);
        let popup_row = if rows_below >= visible {
            cursor_screen_row + 1
        } else {
            cursor_screen_row.saturating_sub(visible).max(1)
        };
        completion_overlay_start = Some(popup_row.saturating_sub(1));
        for (row, &entry) in completion.matches[first..first + visible]
            .iter()
            .enumerate()
        {
            write!(
                &mut editor.frame,
                "\x1b[{};{}H",
                popup_row + row,
                popup_column
            )
            .unwrap();
            editor
                .frame
                .extend_from_slice(if first + row == completion.selected {
                    theme.picker_selected
                } else {
                    theme.status
                });
            let name = completion_name(completion, entry);
            editor.frame.push(b' ');
            let name_available = name_width.saturating_sub(2 + usize::from(scrollbar));
            append_terminal_text(name, name_available, &mut editor.frame);
            let rendered_name = terminal_text_width(name, name_available);
            append_spaces(
                name_width.saturating_sub(1 + rendered_name + usize::from(scrollbar)),
                &mut editor.frame,
            );
            if scrollbar {
                editor
                    .frame
                    .extend_from_slice(if row == scrollbar_row { "█" } else { "│" }.as_bytes());
            }
        }
        if detail_width > 0 {
            for row in 0..visible {
                write!(
                    &mut editor.frame,
                    "\x1b[{};{}H",
                    popup_row + row,
                    popup_column + name_width + 1
                )
                .unwrap();
                editor.frame.extend_from_slice(theme.normal);
                editor.frame.extend_from_slice(b"\x1b[48;2;0;0;0m");
                let detail_line = text_line_at(detail, row);
                editor.frame.push(b' ');
                let detail_rendered = append_rust_completion_text(
                    detail_line.as_bytes(),
                    detail_width - 1,
                    None,
                    theme,
                    true,
                    &mut editor.frame,
                );
                append_spaces(
                    detail_width.saturating_sub(1 + detail_rendered),
                    &mut editor.frame,
                );
            }
        }
        editor.frame.extend_from_slice(theme.normal);
    }
    let screen_row = cursor_line.saturating_sub(top_line) + 1;
    let screen_column = (gutter_width + preview_cursor_column).min(width.saturating_sub(1)) + 1;
    if let Some(search) = search {
        let prompt_column = 2 + search.query.chars().count();
        write!(
            &mut editor.frame,
            "\x1b[{};{}H\x1b[6 q\x1b[?25h",
            height,
            prompt_column.min(width)
        )
        .unwrap();
    } else if mode == Mode::Insert {
        write!(
            &mut editor.frame,
            "\x1b[{};{}H\x1b[6 q\x1b[?25h",
            screen_row, screen_column
        )
        .unwrap();
    }
    let overlay_start = if !key_hints.is_empty() {
        Some(content_height.saturating_sub(key_hints.len().min(content_height)))
    } else {
        completion_overlay_start
    };
    editor.frame.extend_from_slice(b"\x1b[?7h\x1b[?2026l");
    editor_present_document_rows(
        editor,
        terminal,
        &row_ranges,
        status_start,
        overlay_start,
        force,
    )
}

fn text_line_at(text: &str, wanted: usize) -> &str {
    let mut start = 0;
    let mut line = 0;
    for (position, character) in text.char_indices() {
        if character != '\n' {
            continue;
        }
        if line == wanted {
            return &text[start..position];
        }
        line += 1;
        start = position + 1;
    }
    if line == wanted { &text[start..] } else { "" }
}

fn editor_present_document_rows(
    editor: &mut Editor,
    terminal: &mut Terminal,
    row_ranges: &[std::ops::Range<usize>],
    status_start: usize,
    overlay_start: Option<usize>,
    force: bool,
) -> std::io::Result<()> {
    profiling::function_scope!();
    let dirty_overlay_start = match (editor.rendered_overlay_start, overlay_start) {
        (Some(previous), Some(current)) => Some(previous.min(current)),
        (Some(previous), None) => Some(previous),
        (None, Some(current)) => Some(current),
        (None, None) => None,
    };
    editor.rendered_overlay_start = overlay_start;
    let size_changed = editor.rendered_row_hashes.len() != row_ranges.len();
    if force || size_changed {
        editor.rendered_row_hashes.clear();
        editor.rendered_row_hashes.reserve(row_ranges.len());
        for range in row_ranges {
            editor
                .rendered_row_hashes
                .push(idno_std::utils::hash_rapid_bytes(
                    &editor.frame[range.clone()],
                ));
        }
        return terminal::terminal_present(terminal, &editor.frame);
    }
    editor.present_frame.clear();
    editor
        .present_frame
        .extend_from_slice(b"\x1b[?2026h\x1b[?7l\x1b[?25l");
    for (row, range) in row_ranges.iter().enumerate() {
        let hash = idno_std::utils::hash_rapid_bytes(&editor.frame[range.clone()]);
        let overlay_dirty = dirty_overlay_start.is_some_and(|start| row >= start);
        if hash != editor.rendered_row_hashes[row] || overlay_dirty {
            write!(&mut editor.present_frame, "\x1b[{};1H", row + 1).unwrap();
            editor
                .present_frame
                .extend_from_slice(&editor.frame[range.clone()]);
        }
        editor.rendered_row_hashes[row] = hash;
    }
    editor
        .present_frame
        .extend_from_slice(&editor.frame[status_start..]);
    terminal::terminal_present(terminal, &editor.present_frame)
}

fn buffer_terminal_column(buffer: &GapBuffer, position: usize, tab_width: usize) -> usize {
    profiling::function_scope!();
    let mut byte_position = buffer_line_start(buffer, position);
    let mut column = 0;
    while byte_position < position.min(buffer_len(buffer)) {
        let byte = buffer_byte(buffer, byte_position);
        if byte == b'\t' {
            column += tab_width - column % tab_width;
            byte_position += 1;
        } else if byte.is_ascii() {
            column += 1;
            byte_position += 1;
        } else {
            let (character, next) = buffer_decode_char(buffer, byte_position);
            column += terminal::terminal_character_width(character);
            byte_position = next;
        }
    }
    column
}

fn append_buffer_character_within_terminal_width(
    buffer: &GapBuffer,
    position: usize,
    column: usize,
    line_width: usize,
    result: &mut Vec<u8, impl Allocator>,
) -> (usize, usize) {
    let (character, next) = buffer_decode_char(buffer, position);
    let character_width = terminal::terminal_character_width(character);
    if column + character_width <= line_width && (character_width > 0 || column > 0) {
        buffer_append_range(buffer, position, next, result);
    }
    (next, character_width)
}

fn editor_render_picker(
    editor: &mut Editor,
    terminal: &mut Terminal,
    width: usize,
    height: usize,
) -> std::io::Result<()> {
    profiling::function_scope!();
    let picker = editor.picker.as_ref().unwrap();
    let theme = &THEMES[editor.theme];
    let temp = idno_std::mem().scratch().temp();
    let mut label_storage = temp.vec(256);
    let title = match picker.kind {
        PickerKind::Files => "files",
        PickerKind::Commands => "commands",
        PickerKind::SearchProject => "search project",
        PickerKind::DocumentSymbols => "document symbols",
        PickerKind::WorkspaceSymbols => "workspace symbols",
        PickerKind::References => "references",
        PickerKind::DocumentDiagnostics => "document diagnostics",
        PickerKind::WorkspaceDiagnostics => "workspace diagnostics",
    };
    let horizontal_margin = (width / 12).clamp(2, 12);
    let vertical_margin = (height / 10).clamp(1, 4);
    let dialog_width = width.saturating_sub(horizontal_margin * 2).max(1);
    let dialog_height = height.saturating_sub(vertical_margin * 2).max(1);
    let dialog_column = width.saturating_sub(dialog_width) / 2 + 1;
    let dialog_row = height.saturating_sub(dialog_height) / 2 + 1;
    let content_width = dialog_width.saturating_sub(2);
    let result_rows = dialog_height.saturating_sub(4);
    let split = picker.preview.is_some() && content_width >= 60;
    let list_width = if split {
        content_width * 2 / 5
    } else {
        content_width
    };
    let preview_width = content_width.saturating_sub(list_width + usize::from(split));
    let visible_len = picker_visible_len(editor, picker);
    let first = if picker.selected < picker.first_visible {
        picker.selected
    } else if picker.selected >= picker.first_visible + result_rows.max(1) {
        picker.selected + 1 - result_rows.max(1)
    } else {
        picker.first_visible
    };
    let scrollbar = visible_len > result_rows && list_width >= 3;
    let scrollbar_row = if scrollbar {
        first.saturating_mul(result_rows.saturating_sub(1))
            / visible_len.saturating_sub(result_rows).max(1)
    } else {
        0
    };

    editor.frame.clear();
    editor
        .frame
        .extend_from_slice(b"\x1b[?2026h\x1b[?7l\x1b[?25l");
    editor.frame.extend_from_slice(theme.cursor_color);
    editor.frame.extend_from_slice(theme.status);
    picker_render_border_row(
        &mut editor.frame,
        dialog_row,
        dialog_column,
        dialog_width,
        title,
        visible_len,
    );
    picker_render_text_row(
        &mut editor.frame,
        dialog_row + 1,
        dialog_column,
        content_width,
        "> ",
        &picker.query,
        theme.status,
    );
    picker_render_separator_row(
        &mut editor.frame,
        dialog_row + 2,
        dialog_column,
        dialog_width,
        split.then_some(list_width),
        false,
    );
    for visible in 0..result_rows {
        let match_position = first + visible;
        let item = picker_item_at(editor, picker, match_position);
        write!(
            &mut editor.frame,
            "\x1b[{};{}H",
            dialog_row + 3 + visible,
            dialog_column
        )
        .unwrap();
        editor.frame.extend_from_slice(theme.status);
        editor.frame.extend_from_slice("│".as_bytes());
        let selected = match_position == picker.selected && item.is_some();
        editor.frame.extend_from_slice(if selected {
            theme.picker_selected
        } else {
            theme.status
        });
        editor
            .frame
            .extend_from_slice(if selected { b"> " } else { b"  " });
        if let Some(item) = item {
            if !selected
                && matches!(
                    picker.kind,
                    PickerKind::DocumentDiagnostics | PickerKind::WorkspaceDiagnostics
                )
                && let Some(diagnostic) = picker
                    .diagnostic_candidates
                    .get(item)
                    .and_then(|&index| editor.diagnostics.published.get(index))
            {
                editor
                    .frame
                    .extend_from_slice(diagnostic_severity_style(diagnostic.severity, theme));
            }
            picker_item_label(
                &editor.project.labels,
                editor.project_search.as_ref(),
                &editor.rust_methods,
                &editor.diagnostics,
                picker,
                item,
                &mut label_storage,
            );
            let label = std::str::from_utf8(&label_storage).unwrap_or("");
            let label_width = list_width.saturating_sub(2 + usize::from(scrollbar));
            append_terminal_text(label, label_width, &mut editor.frame);
            let rendered_label = terminal_text_width(label, label_width);
            append_spaces(
                list_width.saturating_sub(2 + rendered_label + usize::from(scrollbar)),
                &mut editor.frame,
            );
        } else {
            append_spaces(
                list_width.saturating_sub(2 + usize::from(scrollbar)),
                &mut editor.frame,
            );
        }
        if scrollbar {
            editor.frame.extend_from_slice(
                if visible == scrollbar_row {
                    "█"
                } else {
                    "│"
                }
                .as_bytes(),
            );
        }
        editor.frame.extend_from_slice(theme.status);
        if split {
            editor.frame.extend_from_slice("│".as_bytes());
            if let Some(preview) = picker.preview.as_ref() {
                picker_render_syntax_preview_line(
                    preview,
                    visible,
                    result_rows,
                    preview_width,
                    theme,
                    &mut editor.frame,
                );
            } else {
                append_spaces(preview_width, &mut editor.frame);
            }
        }
        editor.frame.extend_from_slice(theme.status);
        editor.frame.extend_from_slice("│".as_bytes());
    }
    picker_render_separator_row(
        &mut editor.frame,
        dialog_row + dialog_height - 1,
        dialog_column,
        dialog_width,
        split.then_some(list_width),
        true,
    );
    let query_column =
        (dialog_column + 3 + terminal_text_width(&picker.query, content_width.saturating_sub(2)))
            .min(dialog_column + dialog_width.saturating_sub(2));
    write!(
        &mut editor.frame,
        "\x1b[{};{}H\x1b[6 q\x1b[?25h",
        dialog_row + 1,
        query_column
    )
    .unwrap();
    editor.frame.extend_from_slice(b"\x1b[?7h\x1b[?2026l");
    terminal::terminal_present(terminal, &editor.frame)
}

fn diagnostic_severity_style(severity: DiagnosticSeverity, theme: &Theme) -> &'static [u8] {
    match severity {
        DiagnosticSeverity::Error => theme.git_removed,
        DiagnosticSeverity::Warning => theme.git_modified,
        DiagnosticSeverity::Info => theme.syntax[SyntaxKind::CommentNote as usize],
    }
}

fn picker_item_at(editor: &Editor, picker: &Picker, position: usize) -> Option<usize> {
    if picker.kind == PickerKind::Files && picker.query.is_empty() {
        (position < editor.project.labels.len()).then_some(position)
    } else if picker.kind == PickerKind::Files || picker.kind == PickerKind::SearchProject {
        picker.search_ranked.get(position).map(|found| found.item)
    } else {
        picker.matches.get(position).map(|found| found.item)
    }
}

fn picker_item_label(
    project_labels: &[String],
    project_search: Option<&SearchCorpus>,
    rust_methods: &RustMethodIndex,
    diagnostics: &Diagnostics,
    picker: &Picker,
    item: usize,
    result: &mut Vec<u8, impl Allocator>,
) {
    profiling::function_scope!();
    result.clear();
    match picker.kind {
        PickerKind::Files => result.extend_from_slice(project_labels[item].as_bytes()),
        PickerKind::Commands => {
            let label = if command_theme_argument(&picker.query).is_some() {
                THEME_NAMES[item]
            } else if command_file_argument(&picker.query).is_some() {
                project_labels[item].as_str()
            } else {
                COMMANDS[item]
            };
            result.extend_from_slice(label.as_bytes());
        }
        PickerKind::SearchProject => {
            let Some(corpus) = project_search else {
                return;
            };
            let line = corpus.lines[item];
            let Some(label) = project_labels.get(line.project_file as usize) else {
                return;
            };
            write!(result, "{label}:{}: ", line.line_number).unwrap();
            result.extend_from_slice(
                &corpus.bytes[line.text_start as usize..line.display_end as usize],
            );
        }
        PickerKind::DocumentSymbols => {
            let line = picker.symbol_corpus.lines[picker.symbol_candidates[item]];
            let source =
                &picker.symbol_corpus.bytes[line.text_start as usize..line.display_end as usize];
            let Some((start, end)) = rust_symbol_name_range(source) else {
                return;
            };
            result.extend_from_slice(&source[start..end]);
        }
        PickerKind::WorkspaceSymbols => {
            let Some(&symbol) = picker.rust_symbol_candidates.get(item) else {
                return;
            };
            result.extend_from_slice(rust_symbol_name(&rust_methods.corpus, symbol).as_bytes());
        }
        PickerKind::References => {
            let line = picker.symbol_corpus.lines[item];
            result.extend_from_slice(
                &picker.symbol_corpus.bytes[line.display_start as usize..line.display_end as usize],
            );
        }
        PickerKind::DocumentDiagnostics | PickerKind::WorkspaceDiagnostics => {
            let diagnostic = picker.diagnostic_candidates[item];
            result.extend_from_slice(diagnostics.published[diagnostic].display.as_bytes());
        }
    }
}

fn picker_preview_location<'a>(
    project_search: Option<&'a SearchCorpus>,
    project_paths: &[std::path::PathBuf],
    rust_methods: &RustMethodIndex,
    diagnostics: &Diagnostics,
    picker: &'a Picker,
    item: Option<usize>,
) -> Option<PickerPreview<'a>> {
    let item = match item {
        Some(item) => item,
        None => return None,
    };
    match picker.kind {
        PickerKind::Files => {
            let corpus = match project_search {
                Some(corpus) => corpus,
                None => return None,
            };
            let start = corpus
                .lines
                .partition_point(|line| line.project_file < item as u32);
            let end = corpus
                .lines
                .partition_point(|line| line.project_file <= item as u32);
            (start < corpus.lines.len() && corpus.lines[start].project_file == item as u32)
                .then_some(PickerPreview {
                    corpus,
                    target: start,
                    file_start: start,
                    file_end: end,
                })
        }
        PickerKind::SearchProject => {
            let corpus = match project_search {
                Some(corpus) => corpus,
                None => return None,
            };
            let line = match corpus.lines.get(item) {
                Some(line) => *line,
                None => return None,
            };
            let start = corpus
                .lines
                .partition_point(|candidate| candidate.project_file < line.project_file);
            let end = corpus
                .lines
                .partition_point(|candidate| candidate.project_file <= line.project_file);
            Some(PickerPreview {
                corpus,
                target: item,
                file_start: start,
                file_end: end,
            })
        }
        PickerKind::DocumentSymbols => {
            let line = match picker.symbol_candidates.get(item) {
                Some(line) => *line,
                None => return None,
            };
            Some(PickerPreview {
                corpus: &picker.symbol_corpus,
                target: line,
                file_start: 0,
                file_end: picker.symbol_corpus.lines.len(),
            })
        }
        PickerKind::WorkspaceSymbols => {
            let symbol = match picker.rust_symbol_candidates.get(item) {
                Some(symbol) => *symbol,
                None => return None,
            };
            let definition = match rust_methods.corpus.symbols.get(symbol) {
                Some(definition) => *definition,
                None => return None,
            };
            let path = match rust_methods.corpus.paths.get(definition.path as usize) {
                Some(path) => path,
                None => return None,
            };
            let project_file = match project_paths.iter().position(|candidate| candidate == path) {
                Some(project_file) => project_file,
                None => return None,
            };
            let corpus = match project_search {
                Some(corpus) => corpus,
                None => return None,
            };
            project_preview_location(corpus, project_file, definition.position)
        }
        PickerKind::References => {
            let target = match picker.reference_targets.get(item) {
                Some(target) => *target,
                None => return None,
            };
            if target.project_file == u32::MAX {
                document_preview_location(&picker.preview_corpus, target.start as usize)
            } else {
                let corpus = match project_search {
                    Some(corpus) => corpus,
                    None => return None,
                };
                project_preview_location(corpus, target.project_file as usize, target.start)
            }
        }
        PickerKind::DocumentDiagnostics | PickerKind::WorkspaceDiagnostics => {
            let diagnostic = match picker.diagnostic_candidates.get(item) {
                Some(diagnostic) => *diagnostic,
                None => return None,
            };
            let diagnostic = match diagnostics.published.get(diagnostic) {
                Some(diagnostic) => diagnostic,
                None => return None,
            };
            let project_file = match project_paths
                .iter()
                .position(|candidate| candidate == &diagnostic.path)
            {
                Some(project_file) => project_file,
                None => return None,
            };
            let corpus = match project_search {
                Some(corpus) => corpus,
                None => return None,
            };
            let file_start = corpus
                .lines
                .partition_point(|line| line.project_file < project_file as u32);
            let target = file_start + diagnostic.line as usize;
            project_preview_location(
                corpus,
                project_file,
                corpus.lines.get(target).map_or(0, |line| line.file_offset),
            )
        }
        PickerKind::Commands => None,
    }
}

fn picker_rebuild_preview(editor: &Editor, picker: &mut Picker) {
    profiling::function_scope!();
    let item = match picker_item_at(editor, picker, picker.selected) {
        Some(item) => item,
        None => {
            picker.preview = None;
            return;
        }
    };
    let key = PickerPreviewKey {
        kind: picker.kind,
        item,
    };
    if picker
        .preview
        .as_ref()
        .is_some_and(|preview| preview.key == key)
    {
        return;
    }
    let location = match picker_preview_location(
        editor.project_search.as_ref(),
        &editor.project.paths,
        &editor.rust_methods,
        &editor.diagnostics,
        picker,
        Some(item),
    ) {
        Some(location) => location,
        None => {
            picker.preview = None;
            picker_start_workspace_symbol_preview_load(editor, picker, key, item);
            return;
        }
    };
    let maximum_lines = 256;
    let mut first = location
        .target
        .saturating_sub(maximum_lines / 2)
        .max(location.file_start);
    let end = (first + maximum_lines).min(location.file_end);
    first = end.saturating_sub(maximum_lines).max(location.file_start);
    let temp = idno_std::mem().scratch().temp();
    let mut source = temp.vec(64 * 1024);
    for line in &location.corpus.lines[first..end] {
        source.extend_from_slice(
            &location.corpus.bytes[line.text_start as usize..line.display_end as usize],
        );
        source.push(b'\n');
    }
    let target_line = location.target - first;
    let first_line_number = first - location.file_start + 1;
    let file_offset = location.corpus.lines[first].file_offset as usize;
    let target_range = picker_preview_target_range(editor, picker, item);
    let target_start = target_range.map_or(0, |range| range.0.saturating_sub(file_offset));
    let target_end = target_range.map_or(0, |range| range.1.saturating_sub(file_offset));
    let path = picker_preview_path(editor, picker, item);
    if let Some(task) = picker.preview_load_task.take() {
        task.cancel();
    }
    picker.preview_load_key = None;
    picker_preview_cache_set(
        picker,
        key,
        path,
        &source,
        target_line,
        target_start..target_end,
        first_line_number,
    );
}

fn picker_preview_cache_set(
    picker: &mut Picker,
    key: PickerPreviewKey,
    path: Option<&std::path::Path>,
    source: &[u8],
    target_line: usize,
    target_range: std::ops::Range<usize>,
    first_line_number: usize,
) {
    profiling::function_scope!();
    let mut preview = match picker.preview.take() {
        Some(mut preview) => {
            let length = buffer_len(&preview.buffer);
            buffer_delete(&mut preview.buffer, 0, length);
            buffer_insert(&mut preview.buffer, 0, source);
            preview
        }
        None => PickerPreviewCache {
            key,
            buffer: buffer_from_bytes(source),
            syntax: syntax_highlighting_empty(),
            target_line: 0,
            target_start: 0,
            target_end: 0,
            first_line_number: 1,
        },
    };
    preview.key = key;
    preview.target_line = target_line;
    preview.target_start = target_range.start;
    preview.target_end = target_range.end;
    preview.first_line_number = first_line_number;
    syntax_highlighting_set_path(&mut preview.syntax, path);
    syntax_highlighting_step(
        &preview.buffer,
        &mut preview.syntax,
        256 * 1024,
        std::time::Duration::from_micros(500),
    );
    picker.preview = Some(preview);
}

fn picker_start_workspace_symbol_preview_load(
    editor: &Editor,
    picker: &mut Picker,
    key: PickerPreviewKey,
    item: usize,
) {
    profiling::function_scope!();
    if picker.kind != PickerKind::WorkspaceSymbols || picker.preview_load_key == Some(key) {
        return;
    }
    let Some(&symbol) = picker.rust_symbol_candidates.get(item) else {
        return;
    };
    let Some(definition) = editor.rust_methods.corpus.symbols.get(symbol).copied() else {
        return;
    };
    let Some(path) = rust_symbol_path(&editor.rust_methods.corpus, symbol) else {
        return;
    };
    if let Some(task) = picker.preview_load_task.take() {
        task.cancel();
    }
    let path = path.to_path_buf();
    let target = definition.position as usize;
    let target_end = definition.end as usize;
    picker.preview_load_key = Some(key);
    picker.preview_load_task = Some(
        idno_std::threads().spawn_owned(move || picker_preview_load(key, path, target, target_end)),
    );
}

fn picker_preview_load(
    key: PickerPreviewKey,
    path: std::path::PathBuf,
    target: usize,
    target_end: usize,
) -> PickerPreviewLoad {
    profiling::function_scope!();
    let mut bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => {
            return PickerPreviewLoad {
                key,
                path,
                bytes: Vec::new(),
                target_line: 0,
                target_start: 0,
                target_end: 0,
                first_line_number: 1,
                available: false,
            };
        }
    };
    let target = target.min(bytes.len());
    let target_end = target_end.min(bytes.len());
    let absolute_target_line = bytes[..target]
        .iter()
        .filter(|&&byte| byte == b'\n')
        .count();
    let first_line = absolute_target_line.saturating_sub(128);
    let last_line = first_line + 256;
    let mut line = 0usize;
    let mut first_byte = 0usize;
    let mut last_byte = bytes.len();
    for (position, &byte) in bytes.iter().enumerate() {
        if byte != b'\n' {
            continue;
        }
        line += 1;
        if line == first_line {
            first_byte = position + 1;
        }
        if line == last_line {
            last_byte = position + 1;
            break;
        }
    }
    bytes.copy_within(first_byte..last_byte, 0);
    bytes.truncate(last_byte - first_byte);
    PickerPreviewLoad {
        key,
        path,
        bytes,
        target_line: absolute_target_line - first_line,
        target_start: target.saturating_sub(first_byte),
        target_end: target_end.saturating_sub(first_byte),
        first_line_number: first_line + 1,
        available: true,
    }
}

fn picker_preview_path<'a>(
    editor: &'a Editor,
    picker: &Picker,
    item: usize,
) -> Option<&'a std::path::Path> {
    match picker.kind {
        PickerKind::Files => editor
            .project
            .paths
            .get(item)
            .map(std::path::PathBuf::as_path),
        PickerKind::SearchProject => {
            let corpus = match editor.project_search.as_ref() {
                Some(corpus) => corpus,
                None => return None,
            };
            let line = match corpus.lines.get(item) {
                Some(line) => line,
                None => return None,
            };
            editor
                .project
                .paths
                .get(line.project_file as usize)
                .map(std::path::PathBuf::as_path)
        }
        PickerKind::DocumentSymbols => editor_document(editor).path.as_deref(),
        PickerKind::WorkspaceSymbols => {
            let symbol = match picker.rust_symbol_candidates.get(item) {
                Some(symbol) => *symbol,
                None => return None,
            };
            rust_symbol_path(&editor.rust_methods.corpus, symbol)
        }
        PickerKind::References => {
            let target = match picker.reference_targets.get(item) {
                Some(target) => target,
                None => return None,
            };
            if target.project_file == u32::MAX {
                editor_document(editor).path.as_deref()
            } else {
                editor
                    .project
                    .paths
                    .get(target.project_file as usize)
                    .map(std::path::PathBuf::as_path)
            }
        }
        PickerKind::DocumentDiagnostics | PickerKind::WorkspaceDiagnostics => {
            let diagnostic = match picker.diagnostic_candidates.get(item) {
                Some(diagnostic) => *diagnostic,
                None => return None,
            };
            editor
                .diagnostics
                .published
                .get(diagnostic)
                .map(|diagnostic| diagnostic.path.as_path())
        }
        PickerKind::Commands => None,
    }
}

fn picker_preview_target_range(
    editor: &Editor,
    picker: &Picker,
    item: usize,
) -> Option<(usize, usize)> {
    match picker.kind {
        PickerKind::WorkspaceSymbols => {
            let symbol = match picker.rust_symbol_candidates.get(item) {
                Some(symbol) => *symbol,
                None => return None,
            };
            let definition = match editor.rust_methods.corpus.symbols.get(symbol) {
                Some(definition) => definition,
                None => return None,
            };
            Some((definition.position as usize, definition.end as usize))
        }
        PickerKind::References => {
            let target = match picker.reference_targets.get(item) {
                Some(target) => target,
                None => return None,
            };
            Some((target.start as usize, target.end as usize))
        }
        PickerKind::DocumentSymbols => {
            let line = match picker
                .symbol_candidates
                .get(item)
                .and_then(|&line| picker.symbol_corpus.lines.get(line))
            {
                Some(line) => line,
                None => return None,
            };
            let source =
                &picker.symbol_corpus.bytes[line.text_start as usize..line.display_end as usize];
            let (start, end) = match rust_symbol_name_range(source) {
                Some(range) => range,
                None => return None,
            };
            Some((
                line.file_offset as usize + start,
                line.file_offset as usize + end,
            ))
        }
        PickerKind::DocumentDiagnostics | PickerKind::WorkspaceDiagnostics => {
            let Some(diagnostic) = picker
                .diagnostic_candidates
                .get(item)
                .and_then(|&index| editor.diagnostics.published.get(index))
            else {
                return None;
            };
            let Some(corpus) = editor.project_search.as_ref() else {
                return None;
            };
            let Some(project_file) = editor
                .project
                .paths
                .iter()
                .position(|path| path == &diagnostic.path)
            else {
                return None;
            };
            let file_start = corpus
                .lines
                .partition_point(|line| line.project_file < project_file as u32);
            let Some(start_line) = corpus.lines.get(file_start + diagnostic.line as usize) else {
                return None;
            };
            let Some(end_line) = corpus.lines.get(file_start + diagnostic.end_line as usize) else {
                return None;
            };
            let start_source =
                &corpus.bytes[start_line.text_start as usize..start_line.display_end as usize];
            let end_source =
                &corpus.bytes[end_line.text_start as usize..end_line.display_end as usize];
            let start = start_line.file_offset as usize
                + utf8_character_column_offset(start_source, diagnostic.column as usize);
            let mut end = end_line.file_offset as usize
                + utf8_character_column_offset(end_source, diagnostic.end_column as usize);
            if end <= start {
                end = start
                    + utf8_character_width_at(
                        start_source,
                        utf8_character_column_offset(start_source, diagnostic.column as usize),
                    );
            }
            Some((start, end))
        }
        _ => None,
    }
}

fn utf8_character_column_offset(source: &[u8], column: usize) -> usize {
    profiling::function_scope!();
    let Ok(source) = std::str::from_utf8(source) else {
        return column.min(source.len());
    };
    source
        .char_indices()
        .nth(column)
        .map_or(source.len(), |(position, _)| position)
}

fn utf8_character_width_at(source: &[u8], position: usize) -> usize {
    let Some(&byte) = source.get(position) else {
        return 0;
    };
    if byte < 0x80 {
        1
    } else if byte < 0xe0 {
        2.min(source.len() - position)
    } else if byte < 0xf0 {
        3.min(source.len() - position)
    } else {
        4.min(source.len() - position)
    }
}

fn project_preview_location(
    corpus: &SearchCorpus,
    project_file: usize,
    file_offset: u32,
) -> Option<PickerPreview<'_>> {
    let start = corpus
        .lines
        .partition_point(|line| line.project_file < project_file as u32);
    let end = corpus
        .lines
        .partition_point(|line| line.project_file <= project_file as u32);
    if start == end {
        return None;
    }
    let relative = corpus.lines[start..end]
        .partition_point(|line| line.file_offset <= file_offset)
        .saturating_sub(1);
    Some(PickerPreview {
        corpus,
        target: start + relative,
        file_start: start,
        file_end: end,
    })
}

fn document_preview_location(
    corpus: &SearchCorpus,
    file_offset: usize,
) -> Option<PickerPreview<'_>> {
    if corpus.lines.is_empty() {
        return None;
    }
    let target = corpus
        .lines
        .partition_point(|line| line.file_offset as usize <= file_offset)
        .saturating_sub(1);
    Some(PickerPreview {
        corpus,
        target,
        file_start: 0,
        file_end: corpus.lines.len(),
    })
}

fn picker_render_syntax_preview_line(
    preview: &PickerPreviewCache,
    visible: usize,
    result_rows: usize,
    width: usize,
    theme: &Theme,
    result: &mut Vec<u8, impl Allocator>,
) {
    let total_lines = buffer_line_count(&preview.buffer);
    let maximum_first = total_lines.saturating_sub(result_rows);
    let first = preview
        .target_line
        .saturating_sub(result_rows / 2)
        .min(maximum_first);
    let line = first + visible;
    if line >= total_lines {
        result.extend_from_slice(theme.gutter);
        append_spaces(5.min(width), result);
        if width > 5 {
            result.extend_from_slice("~".as_bytes());
            append_spaces(width - 6, result);
        }
        return;
    }
    let selected_line = line == preview.target_line;
    result.extend_from_slice(theme.gutter);
    if selected_line {
        result.extend_from_slice(theme.preview_line_background);
    }
    let line_number = preview.first_line_number + line;
    let prefix_width = 6.min(width);
    write!(
        result,
        "{:>number_width$} ",
        line_number,
        number_width = prefix_width.saturating_sub(1)
    )
    .unwrap();
    result.extend_from_slice(theme.normal);
    if selected_line {
        result.extend_from_slice(theme.preview_line_background);
    }
    let mut position = buffer_position_at_line_column(&preview.buffer, line, 0);
    let line_end = buffer_line_end(&preview.buffer, position);
    let text_width = width.saturating_sub(prefix_width);
    let spans = syntax_highlighting_spans(&preview.syntax);
    let mut span_position = spans.partition_point(|span| span.end as usize <= position);
    let mut rendered_style = usize::MAX;
    let mut column = 0;
    while position < line_end && column < text_width {
        while span_position < spans.len() && spans[span_position].end as usize <= position {
            span_position += 1;
        }
        let syntax_style = spans
            .get(span_position)
            .filter(|span| span.start as usize <= position && position < span.end as usize)
            .map_or(0, |span| span.kind as usize + 1);
        let symbol_selected = preview.target_start <= position && position < preview.target_end;
        let style = syntax_style | if symbol_selected { 1 << 8 } else { 0 };
        if style != rendered_style {
            result.extend_from_slice(if syntax_style == 0 {
                theme.normal
            } else {
                theme.syntax[syntax_style - 1]
            });
            if symbol_selected {
                result.extend_from_slice(theme.picker_selected);
            } else if selected_line {
                result.extend_from_slice(theme.preview_line_background);
            }
            rendered_style = style;
        }
        let byte = buffer_byte(&preview.buffer, position);
        match byte {
            b'\t' => {
                let spaces = (4 - column % 4).min(text_width - column);
                append_spaces(spaces, result);
                column += spaces;
                position += 1;
            }
            0x00..=0x1f | 0x7f => {
                result.push(b'?');
                column += 1;
                position += 1;
            }
            0x20..=0x7e => {
                result.push(byte);
                column += 1;
                position += 1;
            }
            _ => {
                let (next, character_width) = append_buffer_character_within_terminal_width(
                    &preview.buffer,
                    position,
                    column,
                    text_width,
                    result,
                );
                if column + character_width > text_width {
                    break;
                }
                column += character_width;
                position = next;
            }
        }
    }
    result.extend_from_slice(theme.normal);
    if selected_line {
        result.extend_from_slice(theme.preview_line_background);
    }
    append_spaces(text_width.saturating_sub(column), result);
}

fn picker_render_border_row(
    result: &mut Vec<u8, impl Allocator>,
    row: usize,
    column: usize,
    width: usize,
    title: &str,
    matches: usize,
) {
    write!(result, "\x1b[{row};{column}H").unwrap();
    result.extend_from_slice("┌".as_bytes());
    let inner = width.saturating_sub(2);
    write!(result, " {title}  {matches} ").unwrap();
    let match_digits = if matches == 0 {
        1
    } else {
        matches.ilog10() as usize + 1
    };
    let used = title.len() + match_digits + 4;
    append_repeated_text("─", inner.saturating_sub(used), result);
    if width > 1 {
        result.extend_from_slice("┐".as_bytes());
    }
}

fn picker_render_separator_row(
    result: &mut Vec<u8, impl Allocator>,
    row: usize,
    column: usize,
    width: usize,
    split: Option<usize>,
    bottom: bool,
) {
    write!(result, "\x1b[{row};{column}H").unwrap();
    result.extend_from_slice(if bottom {
        "└".as_bytes()
    } else {
        "├".as_bytes()
    });
    let inner = width.saturating_sub(2);
    if let Some(left) = split {
        append_repeated_text("─", left.min(inner), result);
        if left < inner {
            result.extend_from_slice(if bottom {
                "┴".as_bytes()
            } else {
                "┼".as_bytes()
            });
            append_repeated_text("─", inner - left - 1, result);
        }
    } else {
        append_repeated_text("─", inner, result);
    }
    if width > 1 {
        result.extend_from_slice(if bottom {
            "┘".as_bytes()
        } else {
            "┤".as_bytes()
        });
    }
}

fn picker_render_text_row(
    result: &mut Vec<u8, impl Allocator>,
    row: usize,
    column: usize,
    width: usize,
    prefix: &str,
    text: &str,
    style: &[u8],
) {
    write!(result, "\x1b[{row};{column}H").unwrap();
    result.extend_from_slice(style);
    result.extend_from_slice("│".as_bytes());
    append_terminal_text(prefix, width, result);
    let prefix_width = terminal_text_width(prefix, width);
    append_terminal_text(text, width.saturating_sub(prefix_width), result);
    let used = prefix_width + terminal_text_width(text, width.saturating_sub(prefix_width));
    append_spaces(width.saturating_sub(used), result);
    result.extend_from_slice("│".as_bytes());
}

fn append_terminal_text(text: &str, width: usize, result: &mut Vec<u8, impl Allocator>) {
    let mut column = 0;
    for character in text.chars() {
        let character_width = terminal::terminal_character_width(character);
        if column + character_width > width {
            break;
        }
        if character.is_control() {
            result.push(b'?');
            column += 1;
        } else {
            let mut bytes = [0; 4];
            result.extend_from_slice(character.encode_utf8(&mut bytes).as_bytes());
            column += character_width;
        }
    }
}

fn append_rust_completion_text(
    text: &[u8],
    width: usize,
    selection: Option<std::ops::Range<usize>>,
    theme: &Theme,
    black_background: bool,
    result: &mut Vec<u8, impl Allocator>,
) -> usize {
    profiling::function_scope!();
    let mut position = 0usize;
    let mut column = 0usize;
    let black = b"\x1b[48;2;0;0;0m".as_slice();
    while position < text.len() && column < width {
        let start = position;
        let (end, style) = if text.get(position..position + 3) == Some(b"///")
            || text.get(position..position + 3) == Some(b"//!")
        {
            (text.len(), Some(SyntaxKind::Comment))
        } else if rust_identifier_byte(text[position]) {
            position += 1;
            while position < text.len() && rust_identifier_byte(text[position]) {
                position += 1;
            }
            let word = &text[start..position];
            let mut after = position;
            while after < text.len() && text[after].is_ascii_whitespace() {
                after += 1;
            }
            let style = if matches!(
                word,
                b"pub"
                    | b"fn"
                    | b"mut"
                    | b"const"
                    | b"unsafe"
                    | b"async"
                    | b"where"
                    | b"impl"
                    | b"dyn"
                    | b"self"
                    | b"Self"
            ) {
                SyntaxKind::Keyword
            } else if word.first().is_some_and(u8::is_ascii_uppercase) {
                SyntaxKind::Type
            } else if text.get(after) == Some(&b'(') {
                SyntaxKind::Function
            } else {
                SyntaxKind::Markup
            };
            (position, Some(style))
        } else {
            position += 1;
            let style = if matches!(
                text[start],
                b'(' | b')' | b'[' | b']' | b'{' | b'}' | b',' | b':' | b';'
            ) {
                SyntaxKind::Punctuation
            } else if matches!(text[start], b'&' | b'*' | b'=' | b'+' | b'-' | b'>' | b'<') {
                SyntaxKind::Operator
            } else {
                SyntaxKind::Markup
            };
            (position, Some(style))
        };
        let selected = selection
            .as_ref()
            .is_some_and(|selection| start < selection.end && selection.start < end);
        result.extend_from_slice(style.map_or(theme.normal, |style| theme.syntax[style as usize]));
        if black_background {
            result.extend_from_slice(black);
        }
        if selected {
            result.extend_from_slice(theme.selection_background);
        }
        let available = width - column;
        let token = std::str::from_utf8(&text[start..end]).unwrap_or("");
        append_terminal_text(token, available, result);
        let rendered = terminal_text_width(token, available);
        column += rendered;
        if rendered < terminal_text_width(token, usize::MAX) {
            break;
        }
    }
    column
}

fn terminal_text_width(text: &str, limit: usize) -> usize {
    let mut width = 0;
    for character in text.chars() {
        let character_width =
            terminal::terminal_character_width(character).max(usize::from(character.is_control()));
        if width + character_width > limit {
            break;
        }
        width += character_width;
    }
    width
}

fn append_spaces(count: usize, result: &mut Vec<u8, impl Allocator>) {
    append_repeated(b' ', count, result);
}

fn append_repeated(byte: u8, count: usize, result: &mut Vec<u8, impl Allocator>) {
    result.extend(std::iter::repeat_n(byte, count));
}

fn append_repeated_text(text: &str, count: usize, result: &mut Vec<u8, impl Allocator>) {
    result.reserve(text.len() * count);
    for _ in 0..count {
        result.extend_from_slice(text.as_bytes());
    }
}

fn position_is_selected(
    buffer: &GapBuffer,
    selections: &[SelectionState],
    position: usize,
) -> bool {
    selections.iter().any(|selection| {
        let start = selection.anchor.min(selection.cursor);
        let end = buffer_next_char(buffer, selection.anchor.max(selection.cursor));
        (start..end).contains(&position)
    })
}

fn position_is_visibly_selected(
    buffer: &GapBuffer,
    selections: &[SelectionState],
    position: usize,
    mode: Mode,
) -> bool {
    selections.iter().any(|selection| {
        (mode != Mode::Insert || selection.anchor != selection.cursor)
            && position_is_selected(buffer, std::slice::from_ref(selection), position)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor_with_text(text: &str) -> Editor {
        let mut editor = editor_open(None).unwrap();
        editor.documents[0].buffer = buffer_from_bytes(text.as_bytes());
        editor
    }

    fn contents(editor: &Editor) -> Vec<u8> {
        let mut result = Vec::new();
        buffer_write(&editor_document(editor).buffer, &mut result).unwrap();
        result
    }

    #[test]
    fn undo_and_redo_are_document_local() {
        let mut editor = editor_with_text("one");
        editor_handle_key(&mut editor, Key::Character('d'));
        assert_eq!(contents(&editor), b"ne");
        editor_handle_key(&mut editor, Key::Character('u'));
        assert_eq!(contents(&editor), b"one");
        editor_handle_key(&mut editor, Key::Character('U'));
        assert_eq!(contents(&editor), b"ne");
    }

    #[test]
    fn word_motions_stop_before_helix_punctuation_boundaries() {
        let mut editor = editor_with_text("one two three");
        editor_handle_key(&mut editor, Key::Character('w'));
        assert_eq!(editor.mode, Mode::Normal);
        assert_eq!(editor_document(&editor).anchor, 0);
        assert_eq!(editor_document(&editor).cursor, 3);
        editor_handle_key(&mut editor, Key::Character('w'));
        assert_eq!(editor_document(&editor).anchor, 4);
        assert_eq!(editor_document(&editor).cursor, 7);

        let mut punctuation = editor_with_text("run()");
        editor_handle_key(&mut punctuation, Key::Character('w'));
        assert_eq!(editor_document(&punctuation).anchor, 0);
        assert_eq!(editor_document(&punctuation).cursor, 2);
        editor_handle_key(&mut punctuation, Key::Character('w'));
        assert_eq!(editor_document(&punctuation).anchor, 3);
        assert_eq!(editor_document(&punctuation).cursor, 4);
        editor_handle_key(&mut punctuation, Key::Character('b'));
        assert_eq!(editor_document(&punctuation).anchor, 4);
        assert_eq!(editor_document(&punctuation).cursor, 3);
    }

    #[test]
    fn space_f_opens_file_picker() {
        let mut editor = editor_with_text("");
        editor_handle_key(&mut editor, Key::Character(' '));
        editor_handle_key(&mut editor, Key::Character('f'));
        assert_eq!(
            editor.picker.as_ref().map(|picker| picker.kind),
            Some(PickerKind::Files)
        );
        let picker = editor.picker.as_ref().unwrap();
        assert!(picker.matches.is_empty());
        assert_eq!(
            picker_visible_len(&editor, picker),
            editor.project.labels.len()
        );
    }

    #[test]
    fn opening_a_file_indexes_its_parent_for_the_file_picker() {
        static NEXT_DIRECTORY: std::sync::atomic::AtomicUsize =
            std::sync::atomic::AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "bed-parent-picker-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let readme = directory.join("README.md");
        std::fs::write(&readme, b"readme").unwrap();
        std::fs::write(directory.join("main.rs"), b"fn main() {}").unwrap();

        let mut editor = editor_open(Some(readme)).unwrap();
        editor_handle_key(&mut editor, Key::Character(' '));
        editor_handle_key(&mut editor, Key::Character('f'));
        assert!(editor.project.labels.iter().any(|label| label == "main.rs"));
        assert_eq!(
            editor.project.root,
            std::fs::canonicalize(&directory).unwrap()
        );

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn reload_replaces_modified_content_and_is_undoable() {
        static NEXT_DIRECTORY: std::sync::atomic::AtomicUsize =
            std::sync::atomic::AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "bed-reload-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("reload.txt");
        std::fs::write(&path, b"disk").unwrap();
        let mut editor = editor_open(Some(path.clone())).unwrap();
        editor_handle_key(&mut editor, Key::Character('i'));
        editor_handle_key(&mut editor, Key::Character('x'));
        editor_handle_key(&mut editor, Key::Escape);
        assert_eq!(contents(&editor), b"xdisk");
        std::fs::write(&path, b"fresh").unwrap();

        editor_execute_command(&mut editor, "reload");
        assert_eq!(contents(&editor), b"fresh");
        assert!(!editor_document(&editor).modified);
        editor_handle_key(&mut editor, Key::Character('u'));
        assert_eq!(contents(&editor), b"xdisk");

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn goto_module_definition_selects_the_declaration_before_opening_the_file() {
        static NEXT_DIRECTORY: std::sync::atomic::AtomicUsize =
            std::sync::atomic::AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "bed-module-definition-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let main = directory.join("main.rs");
        let module = directory.join("potato.rs");
        let main_source = b"mod potato;\nfn main() { potato::grow(); }";
        std::fs::write(&main, main_source).unwrap();
        std::fs::write(&module, b"pub fn grow() {}").unwrap();
        let mut editor = editor_open(Some(main)).unwrap();
        let module_use = main_source
            .windows(6)
            .rposition(|word| word == b"potato")
            .unwrap();
        editor.documents[0].cursor = module_use;
        editor.documents[0].anchor = module_use;
        for key in ['g', 'd'] {
            editor_handle_key(&mut editor, Key::Character(key));
        }
        assert_eq!(editor.current, 0);
        assert_eq!(
            (
                editor_document(&editor).cursor,
                editor_document(&editor).anchor
            ),
            (4, 9)
        );
        for key in ['g', 'd'] {
            editor_handle_key(&mut editor, Key::Character(key));
        }
        assert_eq!(
            editor_document(&editor)
                .path
                .as_deref()
                .and_then(std::path::Path::file_name)
                .and_then(std::ffi::OsStr::to_str),
            Some("potato.rs")
        );
        assert_eq!(editor_document(&editor).cursor, 0);
        assert_eq!(
            editor_document(&editor).anchor,
            buffer_previous_char(
                &editor_document(&editor).buffer,
                buffer_len(&editor_document(&editor).buffer)
            )
        );

        editor_switch_document(&mut editor, 0);
        let function_use = main_source
            .windows(4)
            .rposition(|word| word == b"grow")
            .unwrap();
        editor.documents[0].cursor = function_use;
        editor.documents[0].anchor = function_use;
        for key in ['g', 'd'] {
            editor_handle_key(&mut editor, Key::Character(key));
        }
        assert_eq!(
            editor_document(&editor)
                .path
                .as_deref()
                .and_then(std::path::Path::file_name)
                .and_then(std::ffi::OsStr::to_str),
            Some("potato.rs")
        );
        assert_eq!(
            (
                editor_document(&editor).cursor,
                editor_document(&editor).anchor
            ),
            (7, 10)
        );
        editor_handle_key(&mut editor, Key::Control(15));
        assert_eq!(editor.current, 0);
        assert_eq!(editor_document(&editor).cursor, function_use);

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn definition_jumps_center_targets_and_restore_the_original_view() {
        let mut source = String::new();
        for _ in 0..40 {
            source.push('\n');
        }
        let definition = source.len() + 6;
        source.push_str("const TARGET: usize = 1;\n");
        for _ in 0..39 {
            source.push('\n');
        }
        let usage = source.len();
        source.push_str("TARGET\n");
        let mut editor = editor_with_text(&source);
        editor.viewport_height = 20;
        editor.documents[0].path = Some(std::path::PathBuf::from("main.rs"));
        code_index_set_path(
            &mut editor.documents[0].code_index,
            Some(std::path::Path::new("main.rs")),
        );
        editor.documents[0].cursor = usage;
        editor.documents[0].anchor = usage;
        editor.documents[0].top_line = 70;

        for key in ['g', 'd'] {
            editor_handle_key(&mut editor, Key::Character(key));
        }
        assert_eq!(editor_document(&editor).cursor, definition);
        assert_eq!(editor_document(&editor).top_line, 30);

        editor_handle_key(&mut editor, Key::Control(15));
        assert_eq!(editor_document(&editor).cursor, usage);
        assert_eq!(editor_document(&editor).top_line, 70);
    }

    #[test]
    fn every_partial_key_state_has_described_continuations() {
        for pending in [
            PendingKey::Space,
            PendingKey::SpaceTheme,
            PendingKey::Goto,
            PendingKey::Match,
            PendingKey::MatchAround,
            PendingKey::MatchInside,
            PendingKey::MatchSurround,
            PendingKey::MatchReplaceFrom,
            PendingKey::MatchReplaceTo('{'),
            PendingKey::InsertLineAbove,
            PendingKey::InsertLineBelow,
        ] {
            let hints = pending_key_hints(pending);
            assert!(!hints.is_empty(), "missing hints for {pending:?}");
            assert!(
                hints
                    .iter()
                    .all(|hint| !hint.key.is_empty() && !hint.description.is_empty())
            );
        }
    }

    #[test]
    fn secondary_cursor_edits_and_undo_are_one_batch() {
        let mut editor = editor_with_text("a\nb\n");
        editor_handle_key(&mut editor, Key::Character('C'));
        editor_handle_key(&mut editor, Key::Character('i'));
        editor_handle_key(&mut editor, Key::Character('x'));
        assert_eq!(contents(&editor), b"xa\nxb\n");
        assert_eq!(editor_document(&editor).secondary_selections.len(), 1);
        editor_handle_key(&mut editor, Key::Control(26));
        assert_eq!(contents(&editor), b"a\nb\n");
    }

    #[test]
    fn insert_session_is_one_undo_and_redo_transaction() {
        let mut editor = editor_with_text("");
        editor_handle_key(&mut editor, Key::Character('i'));
        for character in "many keys".chars() {
            editor_handle_key(&mut editor, Key::Character(character));
        }
        editor_handle_key(&mut editor, Key::Escape);
        assert_eq!(contents(&editor), b"many keys");
        assert_eq!(editor_document(&editor).undo.len(), 1);

        editor_handle_key(&mut editor, Key::Character('u'));
        assert_eq!(contents(&editor), b"");
        editor_handle_key(&mut editor, Key::Character('U'));
        assert_eq!(contents(&editor), b"many keys");
    }

    #[test]
    fn insert_retains_and_expands_the_original_selection() {
        let mut editor = editor_with_text("word");
        editor.documents[0].anchor = 0;
        editor.documents[0].cursor = 3;
        editor_handle_key(&mut editor, Key::Character('i'));
        editor_handle_key(&mut editor, Key::Character('x'));
        assert_eq!(contents(&editor), b"xword");
        assert_eq!(
            (
                editor_document(&editor).anchor,
                editor_document(&editor).cursor
            ),
            (0, 4)
        );
        editor_handle_key(&mut editor, Key::Escape);
        assert_eq!(
            (
                editor_document(&editor).anchor,
                editor_document(&editor).cursor
            ),
            (4, 0)
        );
    }

    #[test]
    fn insert_exit_uses_the_final_insert_head() {
        let mut editor = editor_with_text("abcd");
        editor_handle_key(&mut editor, Key::Character('i'));
        editor_handle_key(&mut editor, Key::Right);
        editor_handle_key(&mut editor, Key::Right);
        editor_handle_key(&mut editor, Key::Escape);
        assert_eq!(editor_document(&editor).cursor, 2);
    }

    #[test]
    fn comma_keeps_only_the_primary_selection() {
        let mut editor = editor_with_text("a\nb\n");
        editor_handle_key(&mut editor, Key::Character('C'));
        assert_eq!(editor_document(&editor).secondary_selections.len(), 1);
        editor_handle_key(&mut editor, Key::Character(','));
        assert!(editor_document(&editor).secondary_selections.is_empty());
        assert_eq!(editor_document(&editor).cursor, 2);
    }

    #[test]
    fn yank_and_paste_use_internal_multi_value_register() {
        let mut editor = editor_with_text("a\nb\n");
        editor_handle_key(&mut editor, Key::Character('C'));
        editor_handle_key(&mut editor, Key::Character('y'));
        assert_eq!(editor.register.values, [0..1, 1..2]);
        assert_eq!(editor.register.bytes, b"ab");
        editor_handle_key(&mut editor, Key::Character('p'));
        assert_eq!(contents(&editor), b"aa\nbb\n");
        assert_eq!(editor_document(&editor).secondary_selections.len(), 1);
    }

    #[test]
    fn shift_r_replaces_each_selection_from_the_internal_register_as_one_edit() {
        let mut editor = editor_with_text("a\nb\nx\ny\n");
        editor_handle_key(&mut editor, Key::Character('C'));
        editor_handle_key(&mut editor, Key::Character('y'));
        editor.documents[0].cursor = 4;
        editor.documents[0].anchor = 4;
        editor.documents[0].secondary_selections.clear();
        editor.documents[0]
            .secondary_selections
            .push(SelectionState {
                cursor: 6,
                anchor: 6,
            });

        editor_handle_key(&mut editor, Key::Character('R'));

        assert_eq!(contents(&editor), b"a\nb\na\nb\n");
        assert_eq!(editor_document(&editor).secondary_selections.len(), 1);
        assert_eq!(editor_document(&editor).undo.len(), 1);
        document_undo(editor_document_mut(&mut editor));
        assert_eq!(contents(&editor), b"a\nb\nx\ny\n");
    }

    #[test]
    fn system_clipboard_replacement_replaces_every_selection_as_one_edit() {
        let mut editor = editor_with_text("one\ntwo\n");
        editor.documents[0].anchor = 0;
        editor.documents[0].cursor = 2;
        editor.documents[0]
            .secondary_selections
            .push(SelectionState {
                anchor: 4,
                cursor: 6,
            });

        editor_replace_selections(&mut editor, b"value");

        assert_eq!(contents(&editor), b"value\nvalue\n");
        assert_eq!(editor_document(&editor).secondary_selections.len(), 1);
        assert_eq!(editor_document(&editor).undo.len(), 1);
        document_undo(editor_document_mut(&mut editor));
        assert_eq!(contents(&editor), b"one\ntwo\n");
    }

    #[test]
    fn enter_copies_indentation_and_indents_open_scopes() {
        let mut editor = editor_with_text("    {");
        editor_handle_key(&mut editor, Key::Character('A'));
        editor_handle_key(&mut editor, Key::Enter);
        assert_eq!(contents(&editor), b"    {\n        ");
    }

    #[test]
    fn open_line_uses_enclosing_scope_instead_of_continuation_indentation() {
        let source = "fn resolve(name: &str) -> String {\n    std::env::current_dir()\n        .map(|directory| directory.join(name).to_string_lossy().into_owned())\n        .unwrap_or_else(|_| name.to_string());\n}";
        let mut editor = editor_with_text(source);
        editor.documents[0].path = Some(std::path::PathBuf::from("main.rs"));
        syntax_highlighting_set_path(
            &mut editor.documents[0].syntax,
            Some(std::path::Path::new("main.rs")),
        );
        while !editor.documents[0].syntax.complete {
            let document = &mut editor.documents[0];
            syntax_highlighting_step(
                &document.buffer,
                &mut document.syntax,
                usize::MAX,
                std::time::Duration::MAX,
            );
        }
        let continuation = source.find(".unwrap_or_else").unwrap();
        editor.documents[0].cursor = continuation;
        editor.documents[0].anchor = continuation;

        editor_handle_key(&mut editor, Key::Character('o'));

        assert!(contents(&editor).ends_with(b".unwrap_or_else(|_| name.to_string());\n    \n}"));
        assert_eq!(
            buffer_line_and_column(
                &editor.documents[0].buffer,
                editor.documents[0].insertion_points[0]
            ),
            (4, 4)
        );
    }

    #[test]
    fn open_line_retains_an_unfinished_boolean_continuation_indent() {
        let source =
            "fn check() {\n    if first\n        && second\n        && third\n    {\n    }\n}";
        let mut editor = editor_with_text(source);
        editor.documents[0].path = Some(std::path::PathBuf::from("main.rs"));
        syntax_highlighting_set_path(
            &mut editor.documents[0].syntax,
            Some(std::path::Path::new("main.rs")),
        );
        while !editor.documents[0].syntax.complete {
            let document = &mut editor.documents[0];
            syntax_highlighting_step(
                &document.buffer,
                &mut document.syntax,
                usize::MAX,
                std::time::Duration::MAX,
            );
        }
        let continuation = source.find("&& second").unwrap();
        editor.documents[0].cursor = continuation;
        editor.documents[0].anchor = continuation;

        editor_handle_key(&mut editor, Key::Character('o'));

        let insertion = editor.documents[0].insertion_points[0];
        assert_eq!(
            buffer_line_and_column(&editor.documents[0].buffer, insertion),
            (3, 8)
        );
    }

    #[test]
    fn auto_pairs_and_paired_enter_keep_the_cursor_inside() {
        let mut editor = editor_with_text("");
        editor_handle_key(&mut editor, Key::Character('i'));
        editor_handle_key(&mut editor, Key::Character('{'));
        assert_eq!(contents(&editor), b"{}");
        assert_eq!(editor_document(&editor).insertion_points, [1]);
        editor_handle_key(&mut editor, Key::Enter);
        assert_eq!(contents(&editor), b"{\n    \n}");
        assert_eq!(editor_document(&editor).insertion_points, [6]);
        editor_handle_key(&mut editor, Key::Escape);
        assert_eq!(contents(&editor), b"{\n\n}");
        assert_eq!(editor_document(&editor).cursor, 2);
        assert_eq!(editor_document(&editor).anchor, 2);
    }

    #[test]
    fn collapsed_insert_does_not_create_or_expand_a_selection() {
        let mut editor = editor_with_text("word");
        editor_handle_key(&mut editor, Key::Character('i'));
        editor_handle_key(&mut editor, Key::Character('x'));
        assert_eq!(
            (
                editor_document(&editor).anchor,
                editor_document(&editor).cursor
            ),
            (1, 1)
        );
        editor_handle_key(&mut editor, Key::Escape);
        assert_eq!(
            (
                editor_document(&editor).anchor,
                editor_document(&editor).cursor
            ),
            (0, 0)
        );

        editor_handle_key(&mut editor, Key::Character('o'));
        assert_eq!(
            (
                editor_document(&editor).anchor,
                editor_document(&editor).cursor
            ),
            (6, 6)
        );
        editor_handle_key(&mut editor, Key::Escape);
        assert_eq!(
            (
                editor_document(&editor).anchor,
                editor_document(&editor).cursor
            ),
            (6, 6)
        );
    }

    #[test]
    fn backspace_removes_untouched_auto_pair_closers() {
        let mut editor = editor_with_text("");
        editor_handle_key(&mut editor, Key::Character('i'));
        for character in ['[', '(', '{'] {
            editor_handle_key(&mut editor, Key::Character(character));
        }
        assert_eq!(contents(&editor), b"[({})]");
        for _ in 0..3 {
            editor_handle_key(&mut editor, Key::Backspace);
        }
        assert_eq!(contents(&editor), b"");
    }

    #[test]
    fn backspace_treats_complete_space_indentation_units_like_tabs() {
        let mut editor = editor_with_text("        value\n   value");
        editor.documents[0].cursor = 8;
        editor.documents[0].anchor = 8;
        editor_handle_key(&mut editor, Key::Character('i'));
        editor_handle_key(&mut editor, Key::Backspace);
        assert_eq!(contents(&editor), b"    value\n   value");
        assert_eq!(editor_document(&editor).insertion_points, [4]);
        editor_handle_key(&mut editor, Key::Escape);

        editor.documents[0].cursor = 13;
        editor.documents[0].anchor = 13;
        editor_handle_key(&mut editor, Key::Character('i'));
        editor_handle_key(&mut editor, Key::Backspace);
        assert_eq!(contents(&editor), b"    value\n  value");
    }

    #[test]
    fn escape_removes_unused_auto_indentation_from_open_line() {
        let mut editor = editor_with_text("    item");
        editor_handle_key(&mut editor, Key::Character('o'));
        assert_eq!(contents(&editor), b"    item\n    ");
        editor_handle_key(&mut editor, Key::Escape);
        assert_eq!(contents(&editor), b"    item\n");
        assert_eq!(
            (
                editor_document(&editor).anchor,
                editor_document(&editor).cursor
            ),
            (9, 9)
        );
        editor_handle_key(&mut editor, Key::Character('u'));
        assert_eq!(contents(&editor), b"    item");
    }

    #[test]
    fn repeated_enter_keeps_only_the_active_lines_indentation() {
        let mut editor = editor_with_text("    item");
        editor_handle_key(&mut editor, Key::Character('A'));
        editor_handle_key(&mut editor, Key::Enter);
        editor_handle_key(&mut editor, Key::Enter);
        editor_handle_key(&mut editor, Key::Enter);
        assert_eq!(contents(&editor), b"    item\n\n\n    ");
        assert_eq!(editor_document(&editor).insertion_points, [15]);
        editor_handle_key(&mut editor, Key::Character('x'));
        assert_eq!(contents(&editor), b"    item\n\n\n    x");
    }

    #[test]
    fn repeated_enter_after_o_keeps_cursor_on_the_active_indented_line() {
        let mut editor = editor_with_text("fn main() {\n}");
        editor_handle_key(&mut editor, Key::Character('o'));
        editor_handle_key(&mut editor, Key::Enter);
        editor_handle_key(&mut editor, Key::Enter);
        assert_eq!(contents(&editor), b"fn main() {\n\n\n    \n}");
        assert_eq!(editor_document(&editor).insertion_points, [18]);
        assert_eq!(
            buffer_line_and_column(&editor_document(&editor).buffer, 18),
            (3, 4)
        );
    }

    #[test]
    fn repeated_enter_after_i_does_not_leave_indented_intermediate_lines() {
        let mut editor = editor_with_text("    value");
        editor.documents[0].cursor = 4;
        editor.documents[0].anchor = 4;
        editor_handle_key(&mut editor, Key::Character('i'));
        editor_handle_key(&mut editor, Key::Enter);
        editor_handle_key(&mut editor, Key::Enter);
        assert_eq!(contents(&editor), b"\n\n    value");
        assert_eq!(editor_document(&editor).insertion_points, [6]);
        assert_eq!(
            buffer_line_and_column(&editor_document(&editor).buffer, 6),
            (2, 4)
        );
    }

    #[test]
    fn bracket_space_inserts_blank_lines_without_moving_selection() {
        let mut editor = editor_with_text("one\ntwo");
        editor.documents[0].cursor = 4;
        editor.documents[0].anchor = 4;
        for key in ['[', ' '] {
            editor_handle_key(&mut editor, Key::Character(key));
        }
        assert_eq!(contents(&editor), b"one\n\ntwo");
        assert_eq!(editor_document(&editor).cursor, 5);
        assert_eq!(editor_document(&editor).anchor, 5);

        for key in [']', ' '] {
            editor_handle_key(&mut editor, Key::Character(key));
        }
        assert_eq!(contents(&editor), b"one\n\ntwo\n");
        assert_eq!(editor_document(&editor).cursor, 5);
        assert_eq!(editor_document(&editor).anchor, 5);
    }

    #[test]
    fn match_inside_and_around_select_delimiters() {
        let mut editor = editor_with_text("call(one)");
        editor.documents[0].cursor = 6;
        editor.documents[0].anchor = 6;
        for key in ['m', 'i', '('] {
            editor_handle_key(&mut editor, Key::Character(key));
        }
        assert_eq!(
            (
                editor_document(&editor).anchor,
                editor_document(&editor).cursor
            ),
            (5, 7)
        );
        assert_eq!(editor.mode, Mode::Normal);
        for key in ['m', 'a', '('] {
            editor_handle_key(&mut editor, Key::Character(key));
        }
        assert_eq!(
            (
                editor_document(&editor).anchor,
                editor_document(&editor).cursor
            ),
            (4, 8)
        );
        assert_eq!(editor.mode, Mode::Normal);
    }

    #[test]
    fn match_surround_adds_and_replaces_paired_delimiters() {
        let mut editor = editor_with_text("item");
        editor.documents[0].anchor = 0;
        editor.documents[0].cursor = 3;
        for key in ['m', 's', '{'] {
            editor_handle_key(&mut editor, Key::Character(key));
        }
        assert_eq!(contents(&editor), b"{item}");
        assert_eq!(
            (
                editor_document(&editor).anchor,
                editor_document(&editor).cursor
            ),
            (1, 4)
        );
        for key in ['m', 'r', '{', '['] {
            editor_handle_key(&mut editor, Key::Character(key));
        }
        assert_eq!(contents(&editor), b"[item]");
    }

    #[test]
    fn repeated_x_extends_line_selection_downward() {
        let mut editor = editor_with_text("one\ntwo\nthree");
        editor_handle_key(&mut editor, Key::Character('x'));
        assert_eq!(
            (
                editor_document(&editor).anchor,
                editor_document(&editor).cursor
            ),
            (0, 3)
        );
        editor_handle_key(&mut editor, Key::Character('x'));
        assert_eq!(
            (
                editor_document(&editor).anchor,
                editor_document(&editor).cursor
            ),
            (0, 7)
        );
    }

    #[test]
    fn goto_bindings_follow_helix_positions() {
        let mut editor = editor_with_text("  first\nsecond\n  last\n");
        editor.documents[0].cursor = 10;
        editor.documents[0].anchor = 10;

        for key in ['g', 'g'] {
            editor_handle_key(&mut editor, Key::Character(key));
        }
        assert_eq!(editor_document(&editor).cursor, 0);

        for key in ['g', 'e'] {
            editor_handle_key(&mut editor, Key::Character(key));
        }
        assert_eq!(editor_document(&editor).cursor, 15);

        for key in ['g', 's'] {
            editor_handle_key(&mut editor, Key::Character(key));
        }
        assert_eq!(editor_document(&editor).cursor, 17);

        for key in ['g', 'l'] {
            editor_handle_key(&mut editor, Key::Character(key));
        }
        assert_eq!(editor_document(&editor).cursor, 20);

        for key in ['g', 'h'] {
            editor_handle_key(&mut editor, Key::Character(key));
        }
        assert_eq!(editor_document(&editor).cursor, 15);
    }

    #[test]
    fn counted_goto_uses_one_based_line_numbers() {
        let mut source = String::new();
        for line in 1..=120 {
            writeln!(&mut source, "line {line}").unwrap();
        }
        let mut editor = editor_with_text(&source);
        for key in ['g', '1', '0', '0', 'g'] {
            editor_handle_key(&mut editor, Key::Character(key));
        }
        assert_eq!(
            buffer_line_and_column(
                &editor_document(&editor).buffer,
                editor_document(&editor).cursor
            )
            .0,
            99
        );
        for key in ['g', 'g'] {
            editor_handle_key(&mut editor, Key::Character(key));
        }
        assert_eq!(editor_document(&editor).cursor, 0);
    }

    #[test]
    fn goto_last_accessed_document_toggles_documents() {
        let mut editor = editor_with_text("first");
        let mut second = document_empty();
        second.buffer = buffer_from_bytes(b"second");
        editor.documents.push(second);
        editor_switch_document(&mut editor, 1);
        assert_eq!(editor.current, 1);

        for key in ['g', 'a'] {
            editor_handle_key(&mut editor, Key::Character(key));
        }
        assert_eq!(editor.current, 0);

        for key in ['g', 'a'] {
            editor_handle_key(&mut editor, Key::Character(key));
        }
        assert_eq!(editor.current, 1);
    }

    #[test]
    fn rust_definition_and_reference_bindings_use_local_index() {
        let mut editor = editor_with_text("let value = 1; value + value");
        editor.documents[0].path = Some(std::path::PathBuf::from("main.rs"));
        code_index_set_path(
            &mut editor.documents[0].code_index,
            Some(std::path::Path::new("main.rs")),
        );
        editor.documents[0].cursor = 15;
        editor.documents[0].anchor = 15;
        for key in ['g', 'd'] {
            editor_handle_key(&mut editor, Key::Character(key));
        }
        assert_eq!(editor_document(&editor).cursor, 4);

        editor.documents[0].cursor = 15;
        editor.documents[0].anchor = 15;
        for key in ['g', 'r'] {
            editor_handle_key(&mut editor, Key::Character(key));
        }
        assert_eq!(
            editor.picker.as_ref().map(|picker| picker.kind),
            Some(PickerKind::References)
        );
        assert_eq!(
            editor
                .picker
                .as_ref()
                .map(|picker| picker.reference_targets.len()),
            Some(3)
        );
    }

    #[test]
    fn goto_definition_prefers_a_mutable_local_inside_field_access() {
        let source = "fn open() {\n    let mut document = document_empty();\n    code_index_step(\n        &document.buffer,\n        &mut document.code_index,\n    );\n}";
        let mut editor = editor_with_text(source);
        editor.documents[0].path = Some(std::path::PathBuf::from("editor.rs"));
        code_index_set_path(
            &mut editor.documents[0].code_index,
            Some(std::path::Path::new("editor.rs")),
        );
        let use_position = source.find("&document.buffer").unwrap() + 1;
        editor.documents[0].cursor = use_position;
        editor.documents[0].anchor = use_position;
        editor_goto_definition(&mut editor);
        assert_eq!(
            editor_document(&editor).cursor,
            source.find("document =").unwrap()
        );
    }

    #[test]
    fn goto_definition_resolves_the_real_editor_step_project_search_call() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/editor.rs");
        let source = std::fs::read(&path).unwrap();
        let needle = b"| editor_step_project_search(editor)";
        let usage = source
            .windows(needle.len())
            .position(|window| window == needle)
            .unwrap()
            + 2;
        let declaration_needle = b"fn editor_step_project_search";
        let declaration = source
            .windows(declaration_needle.len())
            .position(|window| window == declaration_needle)
            .unwrap()
            + 3;
        let mut editor = editor_open(Some(path)).unwrap();
        editor.documents[0].cursor = usage;
        editor.documents[0].anchor = usage;
        let document = &mut editor.documents[0];
        while !document.code_index.complete {
            code_index_step(
                &document.buffer,
                &mut document.code_index,
                usize::MAX,
                std::time::Duration::MAX,
            );
        }
        let identifier = code_index_identifier_at(&document.code_index, usage).unwrap();
        assert!(
            code_index_definition_for(&document.buffer, &document.code_index, identifier).is_some()
        );
        editor_goto_definition(&mut editor);
        assert_eq!(
            editor_document(&editor).cursor,
            declaration,
            "status: {}",
            editor.status
        );
    }

    #[test]
    fn goto_definition_resolves_same_file_type_in_the_real_editor_run_signature() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/editor.rs");
        let source = std::fs::read(&path).unwrap();
        let usage_needle = b"pub fn editor_run(editor: &mut Editor";
        let usage = source
            .windows(usage_needle.len())
            .position(|window| window == usage_needle)
            .unwrap()
            + usage_needle.len()
            - b"Editor".len();
        let declaration_needle = b"pub struct Editor {";
        let declaration = source
            .windows(declaration_needle.len())
            .position(|window| window == declaration_needle)
            .unwrap()
            + b"pub struct ".len();
        let mut editor = editor_open(Some(path)).unwrap();
        editor.documents[0].cursor = usage;
        editor.documents[0].anchor = usage;
        editor_goto_definition(&mut editor);
        assert_eq!(
            editor_document(&editor).cursor,
            declaration,
            "status: {}",
            editor.status
        );
    }

    #[test]
    fn qualified_names_match_symbols_within_their_module_or_crate_subtree() {
        assert!(rust_path_module_matches(
            std::path::Path::new("library/std/src/io/error.rs"),
            b"io"
        ));
        assert!(rust_path_module_matches(
            std::path::Path::new("workspace/bitfield/src/lib.rs"),
            b"bitfield"
        ));
        assert!(!rust_path_module_matches(
            std::path::Path::new("workspace/other/src/lib.rs"),
            b"bitfield"
        ));
    }

    #[test]
    fn primitive_types_use_canonical_standard_library_document_symbols() {
        assert_eq!(
            rust_primitive_document_symbol(b"bool"),
            Some(b"prim_bool".as_slice())
        );
        assert_eq!(
            rust_primitive_document_symbol(b"usize"),
            Some(b"prim_usize".as_slice())
        );
        assert_eq!(rust_primitive_document_symbol(b"Potato"), None);
    }

    #[test]
    fn type_alias_targets_follow_qualified_paths_and_default_generics() {
        let source = b"type Flag<T = usize> = core::primitive::bool;";
        let buffer = buffer_from_bytes(source);
        let mut owner = Vec::new();
        let mut name = Vec::new();
        assert!(rust_type_alias_target(
            &buffer,
            b"type Flag".len(),
            &mut owner,
            &mut name,
        ));
        assert_eq!(owner, b"primitive");
        assert_eq!(name, b"bool");
    }

    #[test]
    fn glob_imports_retain_the_enum_owner_for_unqualified_variants() {
        let source = b"use ItemData::*;\nmatch item { ScarfPiece(_) => {} }";
        let buffer = buffer_from_bytes(source);
        let mut owners = Vec::new();
        rust_glob_import_owners(&buffer, source.len(), &mut owners);
        assert!(rust_owner_imported_by_glob(&owners, b"ItemData"));
        assert!(!rust_owner_imported_by_glob(&owners, b"OtherData"));
    }

    #[test]
    fn use_tree_items_retain_their_imported_module_namespace() {
        let source = b"use crate::code_index::{\n    CodeIndex, code_index_step,\n};\nfn run(index: CodeIndex) { code_index_step(index); }";
        let buffer = buffer_from_bytes(source);
        let mut index = code_index_empty();
        code_index_set_path(&mut index, Some(std::path::Path::new("editor.rs")));
        while !index.complete {
            code_index_step(&buffer, &mut index, usize::MAX, std::time::Duration::MAX);
        }
        for name in [b"CodeIndex".as_slice(), b"code_index_step"] {
            for position in source
                .windows(name.len())
                .enumerate()
                .filter_map(|(position, word)| (word == name).then_some(position))
            {
                let identifier = code_index_identifier_at(&index, position).unwrap();
                let mut owner = Vec::new();
                rust_import_owner(&buffer, &index, identifier, &mut owner);
                assert_eq!(owner, b"code_index");
            }
        }
    }

    #[test]
    fn qualified_std_module_and_item_resolve_with_the_same_namespace_model() {
        let corpus = RustMethodCorpus {
            bytes: b"stdioResult".to_vec(),
            methods: Vec::new(),
            symbols: vec![
                crate::rust_methods::RustSymbol {
                    owner_start: 0,
                    owner_end: 0,
                    name_start: 0,
                    name_end: 3,
                    path: 0,
                    position: 0,
                    end: 3,
                    detail_start: 0,
                    detail_end: 0,
                },
                crate::rust_methods::RustSymbol {
                    owner_start: 3,
                    owner_end: 3,
                    name_start: 3,
                    name_end: 5,
                    path: 0,
                    position: 10,
                    end: 12,
                    detail_start: 0,
                    detail_end: 0,
                },
                crate::rust_methods::RustSymbol {
                    owner_start: 5,
                    owner_end: 5,
                    name_start: 5,
                    name_end: 11,
                    path: 1,
                    position: 20,
                    end: 26,
                    detail_start: 0,
                    detail_end: 0,
                },
            ],
            paths: vec![
                std::path::PathBuf::from("/tool/library/std/src/lib.rs"),
                std::path::PathBuf::from("/tool/library/std/src/io/mod.rs"),
            ],
            standard_library_available: true,
        };
        assert_eq!(rust_indexed_symbol(&corpus, b"std", &[], b"io"), Some(1));
        assert_eq!(rust_indexed_symbol(&corpus, b"io", &[], b"Result"), Some(2));
    }

    #[test]
    fn prelude_vec_never_resolves_to_an_unrelated_dependency_type() {
        let symbol = |path| crate::rust_methods::RustSymbol {
            owner_start: 0,
            owner_end: 0,
            name_start: 0,
            name_end: 3,
            path,
            position: 10,
            end: 13,
            detail_start: 0,
            detail_end: 0,
        };
        let corpus = RustMethodCorpus {
            bytes: b"Vec".to_vec(),
            methods: Vec::new(),
            symbols: vec![symbol(0), symbol(1)],
            paths: vec![
                std::path::PathBuf::from("/toolchain/library/alloc/src/vec/mod.rs"),
                std::path::PathBuf::from("/registry/bumpalo/src/collections/vec.rs"),
            ],
            standard_library_available: true,
        };
        assert_eq!(rust_indexed_symbol(&corpus, &[], &[], b"Vec"), Some(0));
    }

    #[test]
    fn imported_function_definition_uses_the_retained_workspace_declaration() {
        static NEXT_DIRECTORY: std::sync::atomic::AtomicUsize =
            std::sync::atomic::AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "bed-imported-definition-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).unwrap();
        let main = directory.join("main.rs");
        let helper = directory.join("helper.rs");
        let other = directory.join("other.rs");
        let source = "use crate::helper::{target};\nfn main() { target(); }\n";
        std::fs::write(&main, source).unwrap();
        std::fs::write(&helper, "pub fn target() {}\n").unwrap();
        std::fs::write(&other, "pub fn target() {}\n").unwrap();
        let mut editor = editor_open(Some(main)).unwrap();
        let usage = source.rfind("target").unwrap();
        editor.documents[0].anchor = usage;
        editor.documents[0].cursor = usage;

        editor_goto_definition(&mut editor);

        assert_eq!(
            editor_document(&editor).path.as_deref(),
            Some(helper.as_path())
        );
        assert_eq!(editor_document(&editor).cursor, 7);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn field_references_do_not_bind_same_named_function_locals() {
        let source = "struct IgnoreRule { pub pattern: usize }\nfn unrelated(pattern: usize) { consume(pattern); }\nfn apply(rule: IgnoreRule) { consume(rule.pattern); }";
        let mut editor = editor_with_text(source);
        editor.project_search = None;
        editor.documents[0].path = Some(std::path::PathBuf::from("main.rs"));
        code_index_set_path(
            &mut editor.documents[0].code_index,
            Some(std::path::Path::new("main.rs")),
        );
        let field = source.find("pattern").unwrap();
        editor.documents[0].cursor = field;
        editor.documents[0].anchor = field;
        for key in ['g', 'r'] {
            editor_handle_key(&mut editor, Key::Character(key));
        }
        assert_eq!(editor.picker.as_ref().unwrap().reference_targets.len(), 2);
    }

    #[test]
    fn field_references_infer_sort_closure_parameters_from_vec_element_type() {
        let source = "struct Diagnostic { pub severity: usize }\nstruct DiagnosticsResult { diagnostics: Vec<Diagnostic> }\nfn collect() { let mut diagnostics = Vec::new(); diagnostics.sort_unstable_by(|left, right| { left.severity.cmp(&right.severity) }); }";
        let mut editor = editor_with_text(source);
        editor.documents[0].path = Some(std::path::PathBuf::from("diagnostics.rs"));
        code_index_set_path(
            &mut editor.documents[0].code_index,
            Some(std::path::Path::new("diagnostics.rs")),
        );
        let field = source.find("severity").unwrap();
        editor.documents[0].cursor = field;
        editor.documents[0].anchor = field;
        editor_select_references(&mut editor);
        assert_eq!(
            editor
                .picker
                .as_ref()
                .map(|picker| picker.reference_targets.len()),
            Some(3)
        );
    }

    #[test]
    fn real_diagnostic_severity_field_finds_sort_closure_references() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/diagnostics.rs");
        let source = std::fs::read(&path).unwrap();
        let field = source
            .windows(b"pub severity".len())
            .position(|window| window == b"pub severity")
            .unwrap()
            + 4;
        let mut editor = editor_open(Some(path)).unwrap();
        editor.documents[0].cursor = field;
        editor.documents[0].anchor = field;
        editor_select_references(&mut editor);
        let local_references = editor
            .picker
            .as_ref()
            .unwrap()
            .reference_targets
            .iter()
            .filter(|target| target.project_file == u32::MAX)
            .count();
        assert!(
            local_references >= 3,
            "found {local_references} local references"
        );
    }

    #[test]
    fn reference_picker_excludes_comment_and_string_mentions() {
        let source = "let value = 1; // value\nlet text = \"value\";\nvalue";
        let mut editor = editor_with_text(source);
        editor.documents[0].path = Some(std::path::PathBuf::from("main.rs"));
        code_index_set_path(
            &mut editor.documents[0].code_index,
            Some(std::path::Path::new("main.rs")),
        );
        editor.documents[0].cursor = 4;
        editor.documents[0].anchor = 4;
        for key in ['g', 'r'] {
            editor_handle_key(&mut editor, Key::Character(key));
        }
        assert_eq!(editor.picker.as_ref().unwrap().reference_targets.len(), 2);
        assert!(!rust_line_position_is_code(b"// value", 3));
        assert!(!rust_line_position_is_code(b"\"value\"", 2));
        assert!(rust_line_position_is_code(b"value: &'a str", 0));
    }

    #[test]
    fn local_reference_picker_keeps_only_the_same_lexical_binding() {
        let source =
            "fn one(size: usize) { use_value(size); }\nfn two(size: usize) { use_value(size); }";
        let mut editor = editor_with_text(source);
        editor.documents[0].path = Some(std::path::PathBuf::from("main.rs"));
        code_index_set_path(
            &mut editor.documents[0].code_index,
            Some(std::path::Path::new("main.rs")),
        );
        let first_parameter = source.find("size").unwrap();
        editor.documents[0].cursor = first_parameter;
        editor.documents[0].anchor = first_parameter;

        for key in ['g', 'r'] {
            editor_handle_key(&mut editor, Key::Character(key));
        }

        assert_eq!(editor.picker.as_ref().unwrap().reference_targets.len(), 2);
    }

    #[test]
    fn retained_rust_occurrences_exclude_comments_and_strings() {
        let source = b"fn run() { run(); /* run */ let text = \"run\"; }\n// run\nrun();";
        let mut identifiers = Vec::new();
        rust_source_identifiers(source, &mut identifiers);
        let run_positions: Vec<_> = identifiers
            .iter()
            .filter(|range| &source[range.start as usize..range.end as usize] == b"run")
            .collect();
        assert_eq!(run_positions.len(), 3);
    }

    #[test]
    fn explicit_type_member_completion_accepts_with_enter() {
        let mut editor = editor_with_text("let potato: Vec<usize>; potato.p");
        editor.documents[0].path = Some(std::path::PathBuf::from("main.rs"));
        editor
            .rust_methods
            .corpus
            .bytes
            .extend_from_slice(b"Vecpush");
        editor
            .rust_methods
            .corpus
            .methods
            .push(crate::rust_methods::RustMethod {
                owner_start: 0,
                owner_end: 3,
                name_start: 3,
                name_end: 7,
                path: 0,
                position: 0,
                end: 0,
                detail_start: 0,
                detail_end: 0,
            });
        let end = buffer_len(&editor.documents[0].buffer);
        editor.documents[0].cursor = end;
        editor.documents[0].anchor = end;
        editor_handle_key(&mut editor, Key::Character('i'));
        editor_handle_key(&mut editor, Key::Character('u'));
        assert_eq!(
            editor
                .completion
                .as_ref()
                .map(|completion| completion.matches.len()),
            Some(1)
        );
        editor_handle_key(&mut editor, Key::Tab);
        assert_eq!(contents(&editor), b"let potato: Vec<usize>; potato.pu");
        editor_handle_key(&mut editor, Key::Enter);
        assert_eq!(contents(&editor), b"let potato: Vec<usize>; potato.push");
    }

    #[test]
    fn qualified_enum_completion_previews_top_variant_and_typing_accepts_it() {
        let mut editor = editor_with_text("editor::Mode::");
        editor.documents[0].path = Some(std::path::PathBuf::from("editor.rs"));
        editor
            .rust_methods
            .corpus
            .bytes
            .extend_from_slice(b"ModeModeInsertModeOtherModeTransient");
        editor
            .rust_methods
            .corpus
            .paths
            .push(std::path::PathBuf::from("editor.rs"));
        editor
            .rust_methods
            .corpus
            .paths
            .push(std::path::PathBuf::from("proc-macro2/src/lib.rs"));
        editor.rust_methods.corpus.symbols.extend([
            crate::rust_methods::RustSymbol {
                owner_start: 0,
                owner_end: 0,
                name_start: 0,
                name_end: 4,
                path: 0,
                position: 0,
                end: 4,
                detail_start: 0,
                detail_end: 0,
            },
            crate::rust_methods::RustSymbol {
                owner_start: 4,
                owner_end: 8,
                name_start: 8,
                name_end: 14,
                path: 0,
                position: 10,
                end: 16,
                detail_start: 0,
                detail_end: 0,
            },
            crate::rust_methods::RustSymbol {
                owner_start: 14,
                owner_end: 18,
                name_start: 18,
                name_end: 23,
                path: 0,
                position: 20,
                end: 25,
                detail_start: 0,
                detail_end: 0,
            },
            crate::rust_methods::RustSymbol {
                owner_start: 23,
                owner_end: 27,
                name_start: 27,
                name_end: 36,
                path: 1,
                position: 30,
                end: 39,
                detail_start: 0,
                detail_end: 0,
            },
        ]);
        let end = buffer_len(&editor.documents[0].buffer);
        editor.documents[0].cursor = end;
        editor.documents[0].anchor = end;
        editor_handle_key(&mut editor, Key::Character('i'));
        editor_refresh_completion(&mut editor);
        let completion = editor.completion.as_ref().unwrap();
        assert_eq!(completion_name(completion, completion.matches[0]), "Insert");
        assert!(
            completion
                .matches
                .iter()
                .all(|&entry| { completion_name(completion, entry) != "Transient" })
        );
        assert_eq!(completion.selected, 0);
        assert!(!completion.preview);
        editor_handle_key(&mut editor, Key::Tab);
        let completion = editor.completion.as_ref().unwrap();
        assert_eq!(completion.selected, 0);
        assert!(completion.preview);
        assert_eq!(
            completion_insertion(completion, completion.matches[0]),
            b"Insert"
        );
        editor_handle_key(&mut editor, Key::Character(':'));
        assert_eq!(contents(&editor), b"editor::Mode::Insert:");
    }

    #[test]
    fn typed_receiver_completes_a_matching_free_function_as_a_method() {
        let mut editor = editor_with_text("let mut editor: Editor; editor.ru");
        editor.documents[0].path = Some(std::path::PathBuf::from("main.rs"));
        let name = b"editor_run";
        let detail = b"pub fn editor_run(editor: &mut Editor, terminal: &mut Terminal)";
        editor.rust_methods.corpus.bytes.extend_from_slice(name);
        editor.rust_methods.corpus.bytes.extend_from_slice(detail);
        editor
            .rust_methods
            .corpus
            .paths
            .push(std::path::PathBuf::from("editor.rs"));
        editor
            .rust_methods
            .corpus
            .symbols
            .push(crate::rust_methods::RustSymbol {
                owner_start: 0,
                owner_end: 0,
                name_start: 0,
                name_end: name.len() as u32,
                path: 0,
                position: 0,
                end: name.len() as u32,
                detail_start: name.len() as u32,
                detail_end: (name.len() + detail.len()) as u32,
            });
        let end = buffer_len(&editor.documents[0].buffer);
        editor.documents[0].cursor = end;
        editor.documents[0].anchor = end;
        editor_handle_key(&mut editor, Key::Character('i'));
        editor_handle_key(&mut editor, Key::Character('n'));
        let completion = editor.completion.as_ref().unwrap();
        let entry = completion
            .matches
            .iter()
            .copied()
            .find(|&entry| completion_name(completion, entry) == "editor_run")
            .unwrap();
        assert_eq!(
            completion_insertion(completion, entry),
            b"editor_run(&mut editor, &mut terminal)"
        );
    }

    #[test]
    fn space_a_fills_an_empty_qualified_struct_literal() {
        static NEXT_DIRECTORY: std::sync::atomic::AtomicUsize =
            std::sync::atomic::AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "bed-fill-fields-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).unwrap();
        let definition_path = directory.join("editor.rs");
        let definition_source =
            "pub struct Editor {\n    pub documents: Vec<usize>,\n    current: usize,\n}\n";
        std::fs::write(&definition_path, definition_source).unwrap();
        let mut editor = editor_with_text("let a = editor::Editor {\n};");
        let main_path = directory.join("main.rs");
        editor.documents[0].path = Some(main_path.clone());
        code_index_set_path(&mut editor.documents[0].code_index, Some(&main_path));
        editor
            .rust_methods
            .corpus
            .bytes
            .extend_from_slice(b"Editor");
        editor.rust_methods.corpus.paths.push(definition_path);
        editor
            .rust_methods
            .corpus
            .symbols
            .push(crate::rust_methods::RustSymbol {
                owner_start: 0,
                owner_end: 0,
                name_start: 0,
                name_end: 6,
                path: 0,
                position: 11,
                end: 17,
                detail_start: 0,
                detail_end: 0,
            });
        editor.documents[0].cursor = "let a = editor::Editor {".len();
        editor.documents[0].anchor = editor.documents[0].cursor;
        for key in [' ', 'a'] {
            editor_handle_key(&mut editor, Key::Character(key));
        }
        assert_eq!(
            contents(&editor),
            b"let a = editor::Editor {\n    documents: todo!(),\n    current: todo!(),\n};"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn local_name_completion_retains_parameters_and_does_not_move_the_cursor() {
        let mut editor = editor_with_text("fn example(argument: usize) { argu");
        editor.documents[0].path = Some(std::path::PathBuf::from("main.rs"));
        code_index_set_path(
            &mut editor.documents[0].code_index,
            Some(std::path::Path::new("main.rs")),
        );
        let end = buffer_len(&editor.documents[0].buffer);
        editor.documents[0].cursor = end;
        editor.documents[0].anchor = end;
        editor_handle_key(&mut editor, Key::Character('i'));
        editor_handle_key(&mut editor, Key::Character('m'));
        let cursor = editor.documents[0].insertion_points[0];
        assert!(editor.completion.as_ref().is_some_and(|completion| {
            completion
                .matches
                .iter()
                .any(|&entry| completion_name(completion, entry) == "argument")
        }));
        editor_handle_key(&mut editor, Key::Down);
        editor_handle_key(&mut editor, Key::Up);
        editor_handle_key(&mut editor, Key::Tab);
        editor_handle_key(&mut editor, Key::BackTab);
        assert_eq!(editor.documents[0].insertion_points, [cursor]);
    }

    #[test]
    fn call_argument_completion_inserts_the_borrow_required_by_the_parameter() {
        let mut editor = editor_with_text("let mut editor: Editor; editor_run(e");
        editor.documents[0].path = Some(std::path::PathBuf::from("main.rs"));
        code_index_set_path(
            &mut editor.documents[0].code_index,
            Some(std::path::Path::new("main.rs")),
        );
        let name = b"editor_run";
        let detail = b"pub fn editor_run(editor: &mut Editor)";
        editor.rust_methods.corpus.bytes.extend_from_slice(name);
        let detail_start = editor.rust_methods.corpus.bytes.len();
        editor.rust_methods.corpus.bytes.extend_from_slice(detail);
        editor
            .rust_methods
            .corpus
            .paths
            .push(std::path::PathBuf::from("editor.rs"));
        editor
            .rust_methods
            .corpus
            .symbols
            .push(crate::rust_methods::RustSymbol {
                owner_start: 0,
                owner_end: 0,
                name_start: 0,
                name_end: name.len() as u32,
                path: 0,
                position: 0,
                end: name.len() as u32,
                detail_start: detail_start as u32,
                detail_end: (detail_start + detail.len()) as u32,
            });
        let end = buffer_len(&editor.documents[0].buffer);
        editor.documents[0].anchor = end;
        editor.documents[0].cursor = end;
        editor_handle_key(&mut editor, Key::Character('i'));
        let temp = idno_std::mem().scratch().temp();
        let mut expected = temp.vec(32);
        assert_eq!(
            rust_call_argument_type(
                &editor.documents[0].buffer,
                editor.documents[0].insertion_points[0],
                &editor.rust_methods.corpus,
                &mut expected,
            ),
            Some(RustBorrowKind::Mutable)
        );
        assert_eq!(expected, b"Editor");
        editor_refresh_completion(&mut editor);

        editor_handle_key(&mut editor, Key::Tab);
        let completion = editor.completion.as_ref().unwrap();
        assert_eq!(completion_name(completion, completion.matches[0]), "editor");
        assert_eq!(
            completion_insertion(completion, completion.matches[0]),
            b"&mut editor"
        );
        editor_handle_key(&mut editor, Key::Enter);
        assert_eq!(
            contents(&editor),
            b"let mut editor: Editor; editor_run(&mut editor"
        );
    }

    #[test]
    fn callable_completion_builds_borrowed_arguments_and_selects_the_first() {
        let temp = idno_std::mem().scratch().temp();
        let mut insertion = temp.vec(64);
        let selection = rust_callable_insertion(
            "editor_run",
            "pub fn editor_run(editor: &mut Editor, terminal: &mut Terminal)",
            Some("editor"),
            false,
            &mut insertion,
        );
        assert_eq!(insertion, b"editor_run(&mut editor, &mut terminal)");
        assert_eq!(
            &insertion[selection.start as usize..selection.end as usize],
            b"&mut editor"
        );
    }

    #[test]
    fn typing_after_previewed_callable_completion_replaces_the_argument_placeholder() {
        let mut editor = editor_with_text("let mut a: Vec<u32> = vec![0u32];\na.pu");
        editor.documents[0].path = Some(std::path::PathBuf::from("main.rs"));
        let end = buffer_len(&editor.documents[0].buffer);
        editor.documents[0].anchor = end;
        editor.documents[0].cursor = end;
        editor.documents[0].insertion_points.push(end);
        document_begin_transaction(&mut editor.documents[0]);
        editor.mode = Mode::Insert;
        let temp = idno_std::mem().scratch().temp();
        let mut insertion = temp.vec(32);
        let selection = rust_callable_insertion(
            "push",
            "pub fn push(&mut self, value: T)",
            None,
            true,
            &mut insertion,
        );
        let mut completion = Completion {
            bytes: Vec::new(),
            matches: Vec::new(),
            selected: 0,
            prefix_start: end - 2,
            preview: false,
        };
        completion_entry_push(
            &mut completion,
            CompletionCandidate {
                name: "push",
                detail: "pub fn push(&mut self, value: T)",
                insertion: &insertion,
                selection,
                replacement_start: end - 2,
                symbol: u32::MAX,
                flags: 0,
            },
        );
        editor.completion = Some(completion);

        editor_handle_key(&mut editor, Key::Tab);
        editor_handle_key(&mut editor, Key::Character('1'));
        editor_handle_key(&mut editor, Key::Character('2'));
        editor_handle_key(&mut editor, Key::Character('3'));

        assert_eq!(
            contents(&editor),
            b"let mut a: Vec<u32> = vec![0u32];\na.push(123)"
        );
    }

    #[test]
    fn postfix_dbg_tab_rewrites_the_expression() {
        let mut editor = editor_with_text("it.dbg");
        editor.documents[0].path = Some(std::path::PathBuf::from("main.rs"));
        let end = buffer_len(&editor.documents[0].buffer);
        editor.documents[0].anchor = end;
        editor.documents[0].cursor = end;
        editor_handle_key(&mut editor, Key::Character('i'));
        editor_handle_key(&mut editor, Key::Character('!'));
        assert_eq!(
            editor
                .completion
                .as_ref()
                .map(|completion| completion.matches.len()),
            Some(1)
        );
        editor_handle_key(&mut editor, Key::Tab);
        assert_eq!(contents(&editor), b"dbg!(it)");
        assert!(editor.completion.is_none());
    }

    #[test]
    fn argument_motions_select_adjacent_call_arguments() {
        let source = "call(one, two, three)";
        let mut editor = editor_with_text(source);
        editor.documents[0].anchor = source.find("one").unwrap();
        editor.documents[0].cursor = editor.documents[0].anchor;
        for key in [']', 'a'] {
            editor_handle_key(&mut editor, Key::Character(key));
        }
        assert_eq!(
            &source.as_bytes()[editor_document(&editor).anchor..=editor_document(&editor).cursor],
            b"two"
        );
        for key in ['[', 'a'] {
            editor_handle_key(&mut editor, Key::Character(key));
        }
        assert_eq!(
            &source.as_bytes()[editor_document(&editor).anchor..=editor_document(&editor).cursor],
            b"one"
        );
    }

    #[test]
    fn filtered_global_completion_uses_the_compact_match_index() {
        let mut editor = editor_with_text("fn main() {\n}");
        editor.documents[0].path = Some(std::path::PathBuf::from("main.rs"));
        editor
            .rust_methods
            .corpus
            .bytes
            .extend_from_slice(b"zexternal");
        editor.rust_methods.corpus.symbols.extend_from_slice(&[
            crate::rust_methods::RustSymbol {
                owner_start: 0,
                owner_end: 0,
                name_start: 0,
                name_end: 1,
                path: 0,
                position: 0,
                end: 1,
                detail_start: 0,
                detail_end: 0,
            },
            crate::rust_methods::RustSymbol {
                owner_start: 0,
                owner_end: 0,
                name_start: 1,
                name_end: 9,
                path: 0,
                position: 0,
                end: 8,
                detail_start: 0,
                detail_end: 0,
            },
        ]);

        editor_handle_key(&mut editor, Key::Character('o'));
        editor_handle_key(&mut editor, Key::Character('e'));

        assert!(editor.completion.as_ref().is_some_and(|completion| {
            completion
                .matches
                .iter()
                .any(|&entry| completion_name(completion, entry) == "external")
        }));
    }

    #[test]
    fn control_c_toggles_every_line_touched_by_the_selection() {
        let source = "    one\n  two\nthree\n";
        let mut editor = editor_with_text(source);
        editor.documents[0].path = Some(std::path::PathBuf::from("main.rs"));
        editor.documents[0].anchor = source.find("one").unwrap();
        editor.documents[0].cursor = source.find("three").unwrap() + 2;

        editor_handle_key(&mut editor, Key::Control(3));
        assert_eq!(contents(&editor), b"    // one\n  // two\n// three\n");

        editor_handle_key(&mut editor, Key::Control(3));
        assert_eq!(contents(&editor), source.as_bytes());
    }

    #[test]
    fn insert_tab_uses_configured_spaces_and_keeps_a_collapsed_selection() {
        let mut editor = editor_with_text("");
        editor.config.indentation_spaces = 3;
        editor_handle_key(&mut editor, Key::Character('i'));
        editor_handle_key(&mut editor, Key::Tab);
        assert_eq!(contents(&editor), b"   ");
        assert_eq!(editor_document(&editor).insertion_points, [3]);
        assert_eq!(editor_document(&editor).anchor, 3);
        assert_eq!(editor_document(&editor).cursor, 3);
    }

    #[test]
    fn collapsed_insert_selection_never_paints_a_newline_cell() {
        let buffer = buffer_from_bytes(b"\n");
        let selections = [SelectionState {
            anchor: 0,
            cursor: 0,
        }];
        assert!(!position_is_visibly_selected(
            &buffer,
            &selections,
            0,
            Mode::Insert,
        ));
        assert!(position_is_visibly_selected(
            &buffer,
            &selections,
            0,
            Mode::Normal,
        ));
    }

    #[test]
    fn tab_and_backtab_navigate_picker_results_in_both_directions() {
        let mut editor = editor_with_text("");
        editor.terminal_height = 12;
        editor.project.paths = std::sync::Arc::new(vec![
            std::path::PathBuf::from("a"),
            std::path::PathBuf::from("b"),
            std::path::PathBuf::from("c"),
        ]);
        editor.project.labels = std::sync::Arc::new(vec!["a".into(), "b".into(), "c".into()]);
        editor_open_picker(&mut editor, PickerKind::Files);
        editor_handle_key(&mut editor, Key::Tab);
        assert_eq!(editor.picker.as_ref().unwrap().selected, 1);
        editor_handle_key(&mut editor, Key::BackTab);
        assert_eq!(editor.picker.as_ref().unwrap().selected, 0);
        editor_handle_key(&mut editor, Key::BackTab);
        assert_eq!(editor.picker.as_ref().unwrap().selected, 2);
    }

    #[test]
    fn picker_viewport_only_moves_when_selection_crosses_an_edge() {
        let mut editor = editor_with_text("");
        editor.terminal_height = 12;
        editor.project.paths = std::sync::Arc::new(
            (0..10)
                .map(|item| std::path::PathBuf::from(format!("{item}.rs")))
                .collect(),
        );
        editor.project.labels =
            std::sync::Arc::new((0..10).map(|item| format!("{item}.rs")).collect());
        editor_open_picker(&mut editor, PickerKind::Files);
        for _ in 0..7 {
            editor_handle_key(&mut editor, Key::Down);
        }
        assert_eq!(editor.picker.as_ref().unwrap().first_visible, 2);
        editor_handle_key(&mut editor, Key::Up);
        assert_eq!(editor.picker.as_ref().unwrap().first_visible, 2);
    }

    #[test]
    fn document_symbol_picker_selects_name_with_cursor_at_start() {
        let mut editor = editor_with_text("fn alpha() {}\n");
        editor.documents[0].path = Some(std::path::PathBuf::from("main.rs"));
        editor_handle_key(&mut editor, Key::Character(' '));
        editor_handle_key(&mut editor, Key::Character('s'));
        assert_eq!(
            editor.picker.as_ref().map(|picker| picker.kind),
            Some(PickerKind::DocumentSymbols)
        );
        editor_handle_key(&mut editor, Key::Enter);
        assert_eq!(
            (
                editor_document(&editor).cursor,
                editor_document(&editor).anchor
            ),
            (3, 7)
        );
    }

    #[test]
    fn workspace_symbol_picker_builds_a_syntax_highlighted_location_preview() {
        let mut editor = editor_with_text("");
        let path = std::path::PathBuf::from("main.rs");
        editor.project.paths = std::sync::Arc::new(vec![path.clone()]);
        editor.project.labels = std::sync::Arc::new(vec!["main.rs".into()]);
        let source = b"pub fn target() { let value = \"text\"; }";
        editor.project_search = Some(SearchCorpus {
            bytes: source.to_vec(),
            lines: vec![SearchLine {
                project_file: 0,
                file_offset: 0,
                line_number: 1,
                text_start: 0,
                display_start: 0,
                display_end: source.len() as u32,
            }],
            identifiers: Vec::new(),
            symbols: Vec::new(),
        });
        editor
            .rust_methods
            .corpus
            .bytes
            .extend_from_slice(b"target");
        editor.rust_methods.corpus.paths.push(path);
        editor
            .rust_methods
            .corpus
            .symbols
            .push(crate::rust_methods::RustSymbol {
                owner_start: 0,
                owner_end: 0,
                name_start: 0,
                name_end: 6,
                path: 0,
                position: 7,
                end: 13,
                detail_start: 0,
                detail_end: 0,
            });
        editor_open_picker(&mut editor, PickerKind::WorkspaceSymbols);
        let preview = editor.picker.as_ref().unwrap().preview.as_ref().unwrap();
        assert_eq!(preview.syntax.language, crate::syntax::SyntaxLanguage::Rust);
        assert!(!syntax_highlighting_spans(&preview.syntax).is_empty());
    }

    #[test]
    fn diagnostic_picker_and_jump_use_the_exact_unicode_source_span() {
        static NEXT_DIRECTORY: std::sync::atomic::AtomicUsize =
            std::sync::atomic::AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "bed-diagnostic-preview-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("main.rs");
        let source = "fn main() {\n    let café = wrong;\n}\n";
        std::fs::write(&path, source).unwrap();
        let mut editor = editor_open(Some(path.clone())).unwrap();
        editor.project.paths = std::sync::Arc::new(vec![path.clone()]);
        editor.project.labels = std::sync::Arc::new(vec![String::from("main.rs")]);
        editor.project_search = Some(project_search_index(
            &editor.project.paths,
            &editor.project.labels,
        ));
        editor
            .diagnostics
            .published
            .push(crate::diagnostics::Diagnostic {
                path: path.clone(),
                display: String::from("error main.rs:2:16 unknown value"),
                message_start: 19,
                line: 1,
                column: 15,
                end_line: 1,
                end_column: 20,
                severity: DiagnosticSeverity::Error,
            });
        editor_open_picker(&mut editor, PickerKind::DocumentDiagnostics);
        let picker = editor.picker.as_ref().unwrap();
        let item = picker_item_at(&editor, picker, 0).unwrap();
        let range = picker_preview_target_range(&editor, picker, item).unwrap();
        let expected = source.find("wrong").unwrap();
        assert_eq!(range, (expected, expected + "wrong".len()));
        let preview = picker.preview.as_ref().unwrap();
        assert_eq!(preview.target_line, 1);
        assert_eq!(
            (preview.target_start, preview.target_end),
            (expected, expected + "wrong".len())
        );
        editor.picker = None;
        editor_goto_diagnostic(&mut editor, 0);
        assert_eq!(editor_document(&editor).cursor, expected);
        assert_eq!(
            editor_document(&editor).anchor,
            expected + "wrong".len() - 1
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn file_picker_builds_preview_with_the_selected_files_syntax() {
        let mut editor = editor_with_text("");
        editor.project.paths = std::sync::Arc::new(vec![std::path::PathBuf::from("source.rs")]);
        editor.project.labels = std::sync::Arc::new(vec!["source.rs".into()]);
        let source = b"pub fn preview() { let text = \"highlighted\"; }";
        editor.project_search = Some(SearchCorpus {
            bytes: source.to_vec(),
            lines: vec![SearchLine {
                project_file: 0,
                file_offset: 0,
                line_number: 1,
                text_start: 0,
                display_start: 0,
                display_end: source.len() as u32,
            }],
            identifiers: Vec::new(),
            symbols: Vec::new(),
        });

        editor_open_picker(&mut editor, PickerKind::Files);

        let preview = editor.picker.as_ref().unwrap().preview.as_ref().unwrap();
        assert_eq!(preview.syntax.language, crate::syntax::SyntaxLanguage::Rust);
        assert!(!syntax_highlighting_spans(&preview.syntax).is_empty());
    }

    #[test]
    fn new_and_buffer_close_commands_manage_scratch_buffers() {
        let mut editor = editor_with_text("first");
        editor_execute_command(&mut editor, "new");
        assert_eq!(editor.documents.len(), 2);
        assert_eq!(editor.current, 1);
        assert_eq!(contents(&editor), b"");

        editor_handle_key(&mut editor, Key::Character('i'));
        editor_handle_key(&mut editor, Key::Character('x'));
        editor_handle_key(&mut editor, Key::Escape);
        editor_execute_command(&mut editor, "bc");
        assert_eq!(editor.documents.len(), 2);
        assert!(editor.status.contains("unsaved"));
        editor.status.clear();
        editor_execute_command(&mut editor, "bc!");
        assert_eq!(editor.documents.len(), 1);
        assert_eq!(contents(&editor), b"first");

        editor_execute_command(&mut editor, "bca!");
        assert_eq!(editor.documents.len(), 1);
        assert_eq!(contents(&editor), b"");
    }

    #[test]
    fn theme_command_completes_arguments_and_shortcuts_switch_theme() {
        let mut editor = editor_with_text("");
        editor_open_picker(&mut editor, PickerKind::Commands);
        editor.picker.as_mut().unwrap().query.push_str("t noc");
        let mut picker = editor.picker.take().unwrap();
        picker_refresh(&editor, &mut picker);
        picker_complete(&editor, &mut picker);
        assert_eq!(picker.query, "t noctis");
        editor.picker = Some(picker);
        editor_accept_picker(&mut editor);
        assert_eq!(THEMES[editor.theme].name, "noctis");

        editor_handle_key(&mut editor, Key::Character(' '));
        editor_handle_key(&mut editor, Key::Character('t'));
        editor_handle_key(&mut editor, Key::Character('b'));
        assert_eq!(THEMES[editor.theme].name, "bogster");
    }

    #[test]
    fn command_tab_only_moves_selection_until_space_commits_the_command() {
        let mut editor = editor_with_text("");
        editor_open_picker(&mut editor, PickerKind::Commands);
        let initial_query = editor.picker.as_ref().unwrap().query.clone();
        editor_handle_key(&mut editor, Key::Tab);
        assert_eq!(editor.picker.as_ref().unwrap().query, initial_query);
        assert_eq!(editor.picker.as_ref().unwrap().selected, 1);
        editor_handle_key(&mut editor, Key::BackTab);
        assert_eq!(editor.picker.as_ref().unwrap().query, initial_query);
        assert_eq!(editor.picker.as_ref().unwrap().selected, 0);

        for character in "theme".chars() {
            editor_handle_key(&mut editor, Key::Character(character));
        }
        editor_handle_key(&mut editor, Key::Character(' '));
        assert_eq!(editor.picker.as_ref().unwrap().query, "theme ");
    }

    #[test]
    fn command_file_argument_tab_uses_fuzzy_project_paths() {
        let mut editor = editor_with_text("");
        editor.project.labels = std::sync::Arc::from(vec![
            String::from("games/game/src/lib.rs"),
            String::from("libs/math.rs"),
        ]);
        editor.project.paths = std::sync::Arc::from(
            editor
                .project
                .labels
                .iter()
                .map(|label| editor.project.root.join(label))
                .collect::<Vec<_>>(),
        );
        editor_open_picker(&mut editor, PickerKind::Commands);
        editor.picker.as_mut().unwrap().query.push_str("w gmslib");
        let mut picker = editor.picker.take().unwrap();
        picker_refresh(&editor, &mut picker);
        picker_complete(&editor, &mut picker);
        assert_eq!(picker.query, "w games/game/src/lib.rs");
    }

    #[test]
    fn exact_search_in_document_selects_the_matching_span() {
        let mut editor = editor_with_text("alpha\nimportant needle here\nomega\n");
        editor_handle_key(&mut editor, Key::Character('/'));
        for character in "needle".chars() {
            editor_handle_key(&mut editor, Key::Character(character));
        }
        assert_eq!(editor.search.as_ref().unwrap().matches.len(), 1);
        editor_handle_key(&mut editor, Key::Enter);
        assert_eq!(editor_document(&editor).anchor, 16);
        assert_eq!(editor_document(&editor).cursor, 21);

        assert_eq!(
            fuzzy_subsequence_span(b"gapb", b"cargo run -p bed"),
            Some((1, 14))
        );
    }

    #[test]
    fn document_search_does_not_correct_or_skip_query_bytes() {
        let mut editor = editor_with_text("needle n_e_e_d_l_e nede needle");
        editor_handle_key(&mut editor, Key::Character('/'));
        for character in "nede".chars() {
            editor_handle_key(&mut editor, Key::Character(character));
        }
        assert_eq!(editor.search.as_ref().unwrap().matches.len(), 1);
        assert_eq!(editor.search.as_ref().unwrap().matches[0].anchor, 19);
    }

    #[test]
    fn document_search_stays_in_place_and_escape_cancels() {
        let mut editor = editor_with_text("one needle two");
        editor.documents[0].cursor = 4;
        editor.documents[0].anchor = 4;
        editor_handle_key(&mut editor, Key::Character('/'));
        assert!(editor.picker.is_none());
        assert!(editor.search.is_some());
        for character in "needle".chars() {
            editor_handle_key(&mut editor, Key::Character(character));
        }
        assert_eq!(editor.search.as_ref().unwrap().matches.len(), 1);
        editor_handle_key(&mut editor, Key::Escape);
        assert!(editor.search.is_none());
        assert_eq!(editor_document(&editor).cursor, 4);
        assert_eq!(editor_document(&editor).anchor, 4);
    }

    #[test]
    fn document_search_uses_smart_case() {
        let mut editor = editor_with_text("allocator\nALLOCATOR\nAllocator\n");
        editor_handle_key(&mut editor, Key::Character('/'));
        for character in "ALLOC".chars() {
            editor_handle_key(&mut editor, Key::Character(character));
        }
        let search = editor.search.as_ref().unwrap();
        assert_eq!(search.matches.len(), 1);
        assert_eq!(search.matches[0].anchor, 10);

        editor_handle_key(&mut editor, Key::Escape);
        editor_handle_key(&mut editor, Key::Character('/'));
        for character in "alloc".chars() {
            editor_handle_key(&mut editor, Key::Character(character));
        }
        assert_eq!(editor.search.as_ref().unwrap().matches.len(), 3);
    }

    #[test]
    fn document_search_starts_after_cursor_and_wraps() {
        let text = "first hit\nmiddle\nsecond hit\nthird hit\n";
        let mut editor = editor_with_text(text);
        let middle = text.find("middle").unwrap();
        editor.documents[0].cursor = middle;
        editor.documents[0].anchor = middle;
        editor_handle_key(&mut editor, Key::Character('/'));
        for character in "hit".chars() {
            editor_handle_key(&mut editor, Key::Character(character));
        }
        let second = text.find("second hit").unwrap() + "second ".len();
        let third = text.find("third hit").unwrap() + "third ".len();
        let first = text.find("hit").unwrap();
        assert_eq!(
            editor.search.as_ref().unwrap().matches[editor.search.as_ref().unwrap().selected]
                .anchor,
            second
        );

        editor_handle_key(&mut editor, Key::Enter);
        editor_handle_key(&mut editor, Key::Character('n'));
        assert_eq!(editor_document(&editor).anchor, third);
        editor_handle_key(&mut editor, Key::Character('n'));
        assert_eq!(editor_document(&editor).anchor, first);
        editor_handle_key(&mut editor, Key::Character('N'));
        assert_eq!(editor_document(&editor).anchor, third);
    }

    #[test]
    fn selection_search_commits_one_cursor_per_occurrence() {
        let mut editor = editor_with_text("red blue red green red");
        editor.documents[0].anchor = 0;
        editor.documents[0].cursor = 21;
        editor_handle_key(&mut editor, Key::Character('s'));
        for character in "red".chars() {
            editor_handle_key(&mut editor, Key::Character(character));
        }
        assert_eq!(editor.search.as_ref().unwrap().matches.len(), 3);
        editor_handle_key(&mut editor, Key::Enter);
        assert!(editor.search.is_none());
        assert_eq!(editor_document(&editor).secondary_selections.len(), 2);
        assert_eq!(editor_document(&editor).anchor, 19);
        assert_eq!(editor_document(&editor).cursor, 21);
    }

    #[test]
    fn project_search_ranks_in_bounded_persistent_steps() {
        let mut editor = editor_with_text("");
        let mut corpus = SearchCorpus {
            bytes: Vec::new(),
            lines: Vec::new(),
            identifiers: Vec::new(),
            symbols: Vec::new(),
        };
        for item in 0..5000 {
            let start = corpus.bytes.len();
            write!(&mut corpus.bytes, "needle item {item}").unwrap();
            let end = corpus.bytes.len();
            corpus.lines.push(SearchLine {
                project_file: 0,
                file_offset: 0,
                line_number: item + 1,
                text_start: start as u32,
                display_start: start as u32,
                display_end: end as u32,
            });
        }
        editor.project_search = Some(corpus);
        editor.project_search_task = None;
        editor_handle_key(&mut editor, Key::Character(' '));
        editor_handle_key(&mut editor, Key::Character('/'));
        editor_handle_key(&mut editor, Key::Character('n'));
        let picker = editor.picker.as_ref().unwrap();
        assert_eq!(picker.matches.len(), 2048);
        assert!(!picker.search_complete);
        while !editor.picker.as_ref().unwrap().search_complete {
            editor_step_project_search(&mut editor);
        }
        assert_eq!(editor.picker.as_ref().unwrap().matches.len(), 5000);

        editor_handle_key(&mut editor, Key::Character('e'));
        let picker = editor.picker.as_ref().unwrap();
        assert_eq!(picker.matches.len(), 2048);
        assert_eq!(picker.search_scan_position, 5000);
    }

    #[test]
    fn accepted_global_search_selects_the_whole_matching_line() {
        static NEXT_DIRECTORY: std::sync::atomic::AtomicUsize =
            std::sync::atomic::AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "bed-global-line-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("main.rs");
        std::fs::write(&path, b"before\nline needle here\nafter\n").unwrap();
        let mut editor = editor_open(Some(path)).unwrap();
        editor.viewport_height = 1;
        editor_handle_key(&mut editor, Key::Character(' '));
        editor_handle_key(&mut editor, Key::Character('/'));
        for character in "needle".chars() {
            editor_handle_key(&mut editor, Key::Character(character));
        }
        editor_handle_key(&mut editor, Key::Enter);
        assert_eq!(
            (
                editor_document(&editor).anchor,
                editor_document(&editor).cursor
            ),
            (7, 23)
        );
        assert_eq!(editor_document(&editor).top_line, 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn vertical_motion_preserves_the_preferred_column_across_short_lines() {
        let mut editor = editor_with_text("abcdef\nx\nabcdef");
        for _ in 0..5 {
            editor_handle_key(&mut editor, Key::Character('l'));
        }
        assert_eq!(
            buffer_line_and_column(
                &editor_document(&editor).buffer,
                editor_document(&editor).cursor
            ),
            (0, 5)
        );
        editor_handle_key(&mut editor, Key::Character('j'));
        assert_eq!(
            buffer_line_and_column(
                &editor_document(&editor).buffer,
                editor_document(&editor).cursor
            ),
            (1, 0)
        );
        editor_handle_key(&mut editor, Key::Character('j'));
        assert_eq!(
            buffer_line_and_column(
                &editor_document(&editor).buffer,
                editor_document(&editor).cursor
            ),
            (2, 5)
        );
    }

    #[test]
    fn function_objects_choose_outermost_scope_and_navigation_includes_nested_functions() {
        let source = "pub fn outer() {\n    fn inner() {}\n    inner();\n}\nfn next() {}\n";
        let mut editor = editor_with_text(source);
        editor.documents[0].path = Some(std::path::PathBuf::from("main.rs"));
        code_index_set_path(
            &mut editor.documents[0].code_index,
            Some(std::path::Path::new("main.rs")),
        );
        editor.documents[0].cursor = source.find("inner();").unwrap();
        editor.documents[0].anchor = editor.documents[0].cursor;
        for key in ['m', 'a', 'f'] {
            editor_handle_key(&mut editor, Key::Character(key));
        }
        assert_eq!(editor_document(&editor).cursor, 0);
        assert_eq!(
            editor_document(&editor).anchor,
            source.find("\n}\n").unwrap() + 1
        );

        editor.documents[0].cursor = 0;
        editor.documents[0].anchor = 0;
        for key in [']', 'f'] {
            editor_handle_key(&mut editor, Key::Character(key));
        }
        assert_eq!(
            editor_document(&editor).cursor,
            source.find("fn inner").unwrap()
        );
        assert_eq!(editor.mode, Mode::Normal);
    }
}
