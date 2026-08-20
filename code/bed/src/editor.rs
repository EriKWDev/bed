use core::alloc::Allocator;
use std::fmt::Write as _;
use std::io::Read as _;
use std::io::Write as _;

use crate::buffer::*;
use crate::code_index::{
    CodeIndex, code_index_definition_for, code_index_empty, code_index_identifier_at,
    code_index_invalidate, code_index_set_path, code_index_step,
};
use crate::diagnostics::{
    Diagnostics, diagnostics_pending, diagnostics_poll, diagnostics_restart, diagnostics_start,
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
    RustMethodIndex, rust_method_complete, rust_method_definition, rust_method_index_empty,
    rust_method_index_pending, rust_method_index_poll, rust_method_index_start, rust_method_name,
    rust_method_path, rust_symbol_name, rust_symbol_path,
};
use crate::syntax::{
    SYNTAX_KIND_COUNT, SyntaxHighlighting, syntax_highlighting_adjust_edits,
    syntax_highlighting_empty, syntax_highlighting_invalidate, syntax_highlighting_set_path,
    syntax_highlighting_spans, syntax_highlighting_step,
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
        key: "Y",
        description: "yank to clipboard",
    },
    KeyHint {
        key: "P",
        description: "paste clipboard after",
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
];
const MATCH_AROUND_KEY_HINTS: &[KeyHint] = &[KeyHint {
    key: "f/<char>",
    description: "select around delimiter",
}];
const MATCH_INSIDE_KEY_HINTS: &[KeyHint] = &[KeyHint {
    key: "f/<char>",
    description: "select inside delimiter",
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
    pub matches: Vec<FuzzyMatch>,
    pub selected: usize,
    pub prefix_start: usize,
}

pub struct Register {
    pub bytes: Vec<u8>,
    pub values: Vec<std::ops::Range<u32>>,
}

pub struct ClipboardPaste {
    pub bytes: Vec<u8>,
    pub available: bool,
}

pub struct Picker {
    pub kind: PickerKind,
    pub query: String,
    pub matches: Vec<FuzzyMatch>,
    pub selected: usize,
    pub search_query: String,
    pub search_candidates: Vec<usize>,
    pub search_candidate_position: usize,
    pub search_scan_position: usize,
    pub search_complete: bool,
    pub search_seen: Vec<u64>,
    pub search_ranked: Vec<FuzzyMatch>,
    pub symbol_corpus: SearchCorpus,
    pub symbol_candidates: Vec<usize>,
    pub rust_symbol_candidates: Vec<usize>,
    pub reference_targets: Vec<ReferenceTarget>,
    pub diagnostic_candidates: Vec<usize>,
}

#[derive(Clone, Copy)]
pub struct ReferenceTarget {
    pub project_file: u32,
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, Copy)]
pub struct SearchLine {
    pub project_file: u32,
    pub file_offset: u32,
    pub text_start: u32,
    pub display_start: u32,
    pub display_end: u32,
}

pub struct SearchCorpus {
    pub bytes: Vec<u8>,
    pub lines: Vec<SearchLine>,
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
    pub rust_methods: RustMethodIndex,
    pub completion: Option<Completion>,
    pub register: Register,
    pub clipboard_copy_task: Option<idno_std::micropool::OwnedTask<bool>>,
    pub clipboard_paste_task: Option<idno_std::micropool::OwnedTask<ClipboardPaste>>,
    pub diagnostics: Diagnostics,
}

const COMMANDS: [&str; 66] = [
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
    "toggle-auto-indentation",
    "toggle-auto-indent-scopes",
    "toggle-auto-pairs",
];

pub struct Theme {
    pub name: &'static str,
    pub normal: &'static [u8],
    pub gutter: &'static [u8],
    pub status: &'static [u8],
    pub search_scope: &'static [u8],
    pub selection: &'static [u8],
    pub cursor: &'static [u8],
    pub syntax: [&'static [u8]; SYNTAX_KIND_COUNT],
    pub git_added: &'static [u8],
    pub git_modified: &'static [u8],
    pub git_removed: &'static [u8],
    pub picker_selected: &'static [u8],
    pub cursor_color: &'static [u8],
}

const THEME_NAMES: [&str; 5] = [
    "kanagawa",
    "bogster",
    "everforest_light",
    "noctis",
    "kaolin-valley-dark",
];

const THEMES: [Theme; 5] = [
    Theme {
        name: "kanagawa",
        normal: b"\x1b[38;2;220;215;186m\x1b[48;2;31;31;40m",
        gutter: b"\x1b[38;2;114;113;105m\x1b[48;2;31;31;40m",
        status: b"\x1b[38;2;220;215;186m\x1b[48;2;42;42;55m",
        search_scope: b"\x1b[38;2;190;186;164m\x1b[48;2;38;39;50m",
        selection: b"\x1b[38;2;220;215;186m\x1b[48;2;45;79;103m",
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
        search_scope: b"\x1b[38;2;190;190;190m\x1b[48;2;31;31;36m",
        selection: b"\x1b[38;2;240;240;240m\x1b[48;2;62;62;78m",
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
        search_scope: b"\x1b[38;2;105;116;112m\x1b[48;2;240;235;218m",
        selection: b"\x1b[38;2;79;91;88m\x1b[48;2;211;222;203m",
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
        search_scope: b"\x1b[38;2;166;180;190m\x1b[48;2;15;39;50m",
        selection: b"\x1b[38;2;226;232;240m\x1b[48;2;33;73;92m",
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
        search_scope: b"\x1b[38;2;190;186;196m\x1b[48;2;48;44;60m",
        selection: b"\x1b[38;2;239;236;228m\x1b[48;2;73;62;91m",
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
        rust_methods,
        completion: None,
        register: Register {
            bytes: Vec::with_capacity(1024),
            values: Vec::with_capacity(8),
        },
        clipboard_copy_task: None,
        clipboard_paste_task: None,
        diagnostics,
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
            | editor_poll_project_search(editor)
            | editor_step_project_search(editor)
            | editor_step_syntax_highlighting(editor)
            | editor_step_code_index(editor)
            | editor_step_git_gutter(editor)
            | editor_poll_rust_methods(editor)
            | editor_poll_clipboard(editor)
            | diagnostics_poll(&mut editor.diagnostics);
        if input_changed || background_changed {
            match editor_render(editor, terminal) {
                Ok(()) => {}
                Err(error) => return Err(error),
            }
        }
        let timeout = if editor_background_work_pending(editor) {
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
            (PendingKey::Space, Key::Character('Y')) => editor_yank_system(editor),
            (PendingKey::Space, Key::Character('P')) => editor_paste_system(editor),
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
    true
}

pub fn document_undo(document: &mut Document) {
    profiling::function_scope!();
    let Some(transaction) = document.undo.pop() else {
        return;
    };
    syntax_highlighting_invalidate(&mut document.syntax);
    code_index_invalidate(&mut document.code_index);
    git_gutter_invalidate(&mut document.git_gutter);
    for edit in transaction.edits.iter().rev() {
        for atom in edit.atoms.iter().rev() {
            buffer_delete(
                &mut document.buffer,
                atom.after_start,
                atom.after_start + atom.inserted.len(),
            );
            buffer_insert(
                &mut document.buffer,
                atom.after_start,
                &edit.deleted_bytes[atom.deleted.clone()],
            );
        }
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
    syntax_highlighting_invalidate(&mut document.syntax);
    code_index_invalidate(&mut document.code_index);
    git_gutter_invalidate(&mut document.git_gutter);
    for edit in &transaction.edits {
        for atom in edit.atoms.iter().rev() {
            buffer_delete(
                &mut document.buffer,
                atom.before_start,
                atom.before_start + atom.deleted.len(),
            );
            buffer_insert(
                &mut document.buffer,
                atom.before_start,
                &edit.inserted_bytes[atom.inserted.clone()],
            );
        }
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
    let mut scan_position = 0;
    let mut scan_line = 0;
    for replacement in replacements.iter() {
        while scan_position < replacement.start {
            scan_line += usize::from(buffer_byte(&document.buffer, scan_position) == b'\n');
            scan_position += 1;
        }
        let start_line = scan_line;
        while scan_position < replacement.end {
            scan_line += usize::from(buffer_byte(&document.buffer, scan_position) == b'\n');
            scan_position += 1;
        }
        let inserted_lines = inserted_bytes[replacement.inserted.clone()]
            .iter()
            .filter(|&&byte| byte == b'\n')
            .count();
        byte_edits.push((
            replacement.start,
            replacement.end,
            replacement.inserted.len(),
        ));
        line_edits.push((start_line, scan_line, inserted_lines));
    }
    syntax_highlighting_invalidate(&mut document.syntax);
    syntax_highlighting_adjust_edits(&mut document.syntax, &byte_edits);
    code_index_invalidate(&mut document.code_index);
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
    for atom in atoms.iter().rev() {
        buffer_delete(
            &mut document.buffer,
            atom.before_start,
            atom.before_start + atom.deleted.len(),
        );
        buffer_insert(
            &mut document.buffer,
            atom.before_start,
            &inserted_bytes[atom.inserted.clone()],
        );
    }
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
        },
        symbol_candidates: Vec::new(),
        rust_symbol_candidates: Vec::new(),
        reference_targets: Vec::new(),
        diagnostic_candidates: Vec::new(),
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
}

fn editor_handle_picker_key(editor: &mut Editor, key: Key) -> bool {
    match key {
        Key::Escape => editor.picker = None,
        Key::Enter => {
            editor_accept_picker(editor);
            if editor.quit_requested {
                return true;
            }
        }
        Key::Up | Key::Control(16) => {
            let picker = editor.picker.as_mut().unwrap();
            picker.selected = picker.selected.saturating_sub(1);
        }
        Key::Down | Key::Control(14) => {
            let visible_len = picker_visible_len(editor, editor.picker.as_ref().unwrap());
            let picker = editor.picker.as_mut().unwrap();
            picker.selected = (picker.selected + 1).min(visible_len.saturating_sub(1));
        }
        Key::Tab
            if editor.picker.as_ref().map(|picker| picker.kind)
                == Some(PickerKind::SearchProject) =>
        {
            let visible_len = picker_visible_len(editor, editor.picker.as_ref().unwrap());
            let picker = editor.picker.as_mut().unwrap();
            picker.selected = (picker.selected + 1).min(visible_len.saturating_sub(1));
        }
        Key::Tab => {
            let mut picker = editor.picker.take().unwrap();
            picker_complete(editor, &mut picker);
            picker_refresh(editor, &mut picker);
            editor.picker = Some(picker);
        }
        Key::Backspace => {
            let mut picker = editor.picker.take().unwrap();
            picker.query.pop();
            picker_refresh(editor, &mut picker);
            editor.picker = Some(picker);
        }
        Key::Character(character) => {
            let mut picker = editor.picker.take().unwrap();
            picker.query.push(character);
            picker_refresh(editor, &mut picker);
            editor.picker = Some(picker);
        }
        _ => {}
    }
    false
}

fn editor_accept_picker(editor: &mut Editor) {
    let Some(picker) = editor.picker.take() else {
        return;
    };
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
    if editor.picker.as_ref().map(|picker| picker.kind) == Some(PickerKind::Files)
        && discovered_files > previous_files
    {
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
    } else {
        editor.project_discovery = Some(discovery);
    }
    discovered_files > previous_files || complete
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
    if editor.picker.as_ref().map(|picker| picker.kind) == Some(PickerKind::SearchProject) {
        let mut picker = editor.picker.take().unwrap();
        picker_refresh(editor, &mut picker);
        editor.picker = Some(picker);
    }
    true
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
    };
    let temp = idno_std::mem().scratch().temp();
    let mut source = temp.vec(64 * 1024);
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
        let mut start = 0;
        let mut line_number = 1;
        while start <= source.len() {
            let end = source[start..]
                .iter()
                .position(|&byte| byte == b'\n')
                .map_or(source.len(), |offset| start + offset);
            let display_start = corpus.bytes.len();
            write!(&mut corpus.bytes, "{label}:{line_number}: ").unwrap();
            let text_start = corpus.bytes.len();
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
                text_start: text_start as u32,
                display_start: display_start as u32,
                display_end: text_end as u32,
            });
            if end >= source.len() {
                break;
            }
            start = end + 1;
            line_number += 1;
        }
    }
    corpus
}

fn editor_accept_project_search(editor: &mut Editor, picker: &Picker, item: usize) {
    let Some(corpus) = editor.project_search.as_ref() else {
        return;
    };
    let Some(line) = corpus.lines.get(item).copied() else {
        return;
    };
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
        Key::Enter => {
            editor.completion = None;
            editor_insert_newline(editor);
        }
        Key::Tab if editor.completion.is_some() => editor_accept_completion(editor),
        Key::Tab => editor_insert(editor, "\t"),
        Key::Character(character) => {
            if editor.config.flags.contains(EditorFlags::AUTO_PAIRS)
                && editor_insert_auto_pair(editor, character)
            {
                editor_refresh_completion(editor);
                return false;
            }
            let mut encoded = [0; 4];
            editor_insert(editor, character.encode_utf8(&mut encoded));
            editor_refresh_completion(editor);
        }
        Key::Control(14) if editor.completion.is_some() => {
            let completion = editor.completion.as_mut().unwrap();
            completion.selected =
                (completion.selected + 1).min(completion.matches.len().saturating_sub(1));
        }
        Key::Control(16) if editor.completion.is_some() => {
            let completion = editor.completion.as_mut().unwrap();
            completion.selected = completion.selected.saturating_sub(1);
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

fn editor_refresh_completion(editor: &mut Editor) {
    profiling::function_scope!();
    if editor.mode != Mode::Insert
        || editor_document(editor)
            .path
            .as_deref()
            .and_then(std::path::Path::extension)
            .and_then(std::ffi::OsStr::to_str)
            != Some("rs")
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
        matches: Vec::with_capacity(32),
        selected: 0,
        prefix_start: insertion_point,
    });
    let prefix_start = rust_method_complete(
        &editor_document(editor).buffer,
        insertion_point,
        &editor.rust_methods.corpus,
        &mut completion.matches,
    );
    let Some(prefix_start) = prefix_start else {
        return;
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

fn editor_accept_completion(editor: &mut Editor) {
    profiling::function_scope!();
    let Some(completion) = editor.completion.take() else {
        return;
    };
    let Some(found) = completion.matches.get(completion.selected) else {
        return;
    };
    let temp = idno_std::mem().scratch().temp();
    let name = rust_method_name(&editor.rust_methods.corpus, found.item);
    let mut inserted = temp.vec(name.len());
    inserted.extend_from_slice(name.as_bytes());
    let document = editor_document_mut(editor);
    let primary = document
        .insertion_points
        .last()
        .copied()
        .unwrap_or(document.cursor);
    let prefix_length = primary.saturating_sub(completion.prefix_start);
    let mut replacements = temp.vec(document.insertion_points.len());
    for &position in &document.insertion_points {
        replacements.push(Replacement {
            start: position.saturating_sub(prefix_length),
            end: position,
            inserted: 0..inserted.len(),
        });
    }
    document_replace_ranges(document, &mut replacements, &inserted, None);
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
    editor.clipboard_paste_task = Some(idno_std::threads().spawn_owned(clipboard_read));
    editor.status.push_str("reading system clipboard");
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
    editor_paste_values(editor, &paste.bytes, std::slice::from_ref(&range), true);
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
    let was_normal = editor.mode == Mode::Normal;
    let document = editor_document_mut(editor);
    let temp = idno_std::mem().scratch().temp();
    let mut selections = temp.vec(document.secondary_selections.len() + 1);
    document_selections(document, &mut selections);
    for selection in &mut selections {
        if was_normal {
            selection.anchor = selection.cursor;
        }
        selection.cursor = if forward {
            buffer_next_word_start(&document.buffer, selection.cursor)
        } else {
            buffer_previous_word_start(&document.buffer, selection.cursor)
        };
        if selection.cursor == buffer_len(&document.buffer) && selection.cursor > 0 {
            selection.cursor = buffer_previous_char(&document.buffer, selection.cursor);
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
    let mut original_points = temp.vec(document.insertion_points.len());
    let mut cursor_offsets = temp.vec(document.insertion_points.len());
    for &position in &document.insertion_points {
        original_points.push(position);
        indentation.clear();
        if auto_indentation {
            indentation_for_position(
                &document.buffer,
                position,
                scope_indentation,
                indentation_spaces,
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
                    &mut outer_indentation,
                );
            }
            inserted_bytes.push(b'\n');
            inserted_bytes.extend_from_slice(&outer_indentation);
        }
        replacements.push(Replacement {
            start: position,
            end: position,
            inserted: inserted_start..inserted_bytes.len(),
        });
    }
    document_replace_ranges(document, &mut replacements, &inserted_bytes, None);
    document.insertion_points.clear();
    for (&position, &cursor_offset) in original_points.iter().zip(&cursor_offsets) {
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
    indentation: &mut Vec<u8, impl Allocator>,
) {
    let start = buffer_line_start(buffer, position);
    let end = buffer_line_end(buffer, position);
    indentation.clear();
    let mut at = start;
    while at < end && matches!(buffer_byte(buffer, at), b' ' | b'\t') {
        indentation.push(buffer_byte(buffer, at));
        at += 1;
    }
    if scope_indentation {
        let mut last = position.min(end);
        while last > start && matches!(buffer_byte(buffer, last - 1), b' ' | b'\t') {
            last -= 1;
        }
        if last > start && matches!(buffer_byte(buffer, last - 1), b'(' | b'[' | b'{') {
            indentation.extend(std::iter::repeat_n(b' ', indentation_spaces));
        }
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
    let line = diagnostic.line as usize;
    let column = diagnostic.column as usize;
    let before = editor_location(editor);
    let Some(target) = editor_document_target(editor, path) else {
        return;
    };
    editor_switch_document_state(editor, target);
    let start = buffer_position_at_line_column(&editor_document(editor).buffer, line, column);
    let end = buffer_next_char(&editor_document(editor).buffer, start);
    editor_set_symbol_selection(editor, start, end);
    let after = editor_location(editor);
    editor_record_jump(editor, before, after);
}

fn editor_goto_definition(editor: &mut Editor) {
    profiling::function_scope!();
    let temp = idno_std::mem().scratch().temp();
    let mut name = temp.vec(64);
    let local_target = {
        let document = editor_document_mut(editor);
        code_index_step(
            &document.buffer,
            &mut document.code_index,
            256 * 1024,
            std::time::Duration::from_micros(500),
        );
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
        code_index_definition_for(&document.buffer, &document.code_index, identifier_index).map(
            |symbol| {
                let symbol = document.code_index.symbols[symbol];
                let identifier = document.code_index.identifiers[symbol.identifier as usize];
                (
                    identifier.start as usize,
                    identifier.end as usize,
                    symbol.kind,
                )
            },
        )
    };
    if local_target.is_some_and(|(_, _, kind)| kind == crate::code_index::CodeSymbolKind::Module)
        && editor_goto_module_file(editor, &name)
    {
        return;
    }
    if let Some((start, end, _)) = local_target {
        editor_select_symbol(editor, start, end);
        return;
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
            return;
        };
        let path = path.to_path_buf();
        editor_navigate_to_symbol(
            editor,
            &path,
            definition.position as usize,
            definition.end as usize,
        );
        return;
    }
    if editor_goto_workspace_symbol(editor, &name) {
        return;
    }
    if editor_goto_indexed_rust_symbol(editor, &name) {
        return;
    }
    editor.status.push_str("definition not indexed");
}

fn editor_goto_indexed_rust_symbol(editor: &mut Editor, name: &[u8]) -> bool {
    profiling::function_scope!();
    let symbol = editor
        .rust_methods
        .corpus
        .symbols
        .iter()
        .enumerate()
        .find_map(|(index, symbol)| {
            let candidate = &editor.rust_methods.corpus.bytes
                [symbol.name_start as usize..symbol.name_end as usize];
            (candidate == name).then_some(index)
        });
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
    editor_navigate_to_symbol(editor, &path, 0, 0);
    true
}

fn editor_goto_workspace_symbol(editor: &mut Editor, name: &[u8]) -> bool {
    profiling::function_scope!();
    let target = editor.project_search.as_ref().and_then(|corpus| {
        corpus.lines.iter().find_map(|line| {
            let project_file = line.project_file as usize;
            if project_file >= editor.project.labels.len()
                || !editor.project.labels[project_file].ends_with(".rs")
            {
                return None;
            }
            let source = &corpus.bytes[line.text_start as usize..line.display_end as usize];
            let Some((start, end)) = rust_symbol_name_range(source) else {
                return None;
            };
            (source[start..end] == *name).then_some((
                project_file,
                line.file_offset as usize + start,
                line.file_offset as usize + end,
            ))
        })
    });
    let Some((project_file, start, end)) = target else {
        return false;
    };
    let Some(path) = editor.project.paths.get(project_file).cloned() else {
        return false;
    };
    editor_navigate_to_symbol(editor, &path, start, end);
    true
}

fn editor_select_references(editor: &mut Editor) {
    profiling::function_scope!();
    let temp = idno_std::mem().scratch().temp();
    let mut name = temp.vec(64);
    let mut workspace = true;
    {
        let document = editor_document_mut(editor);
        code_index_step(
            &document.buffer,
            &mut document.code_index,
            256 * 1024,
            std::time::Duration::from_micros(500),
        );
        let Some(identifier_index) =
            code_index_identifier_at(&document.code_index, document.cursor)
        else {
            editor.status.push_str("no indexed identifier");
            return;
        };
        let identifier = document.code_index.identifiers[identifier_index];
        if let Some(definition) =
            code_index_definition_for(&document.buffer, &document.code_index, identifier_index)
        {
            workspace = document.code_index.symbols[definition].kind
                != crate::code_index::CodeSymbolKind::Value;
        }
        for position in identifier.start as usize..identifier.end as usize {
            name.push(buffer_byte(&document.buffer, position));
        }
    }
    editor_open_picker(editor, PickerKind::References);
    let mut picker = editor.picker.take().unwrap();
    reference_targets_collect(editor, &name, workspace, &mut picker);
    if picker.reference_targets.is_empty() {
        editor.status.push_str("no references indexed");
        return;
    }
    picker_refresh(editor, &mut picker);
    editor.picker = Some(picker);
}

fn reference_targets_collect(editor: &Editor, name: &[u8], workspace: bool, picker: &mut Picker) {
    profiling::function_scope!();
    let document = editor_document(editor);
    let current_project_file = document.path.as_ref().and_then(|path| {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        editor.project.paths.iter().position(|candidate| {
            std::fs::canonicalize(candidate).unwrap_or_else(|_| candidate.clone()) == path
        })
    });
    let length = buffer_len(&document.buffer);
    let mut line_start = 0;
    let mut line_number = 1;
    while line_start <= length {
        let line_end = buffer_line_end(&document.buffer, line_start);
        let mut position = line_start;
        while position + name.len() <= line_end {
            if buffer_range_matches_identifier(&document.buffer, position, line_end, name) {
                let label_start = picker.symbol_corpus.bytes.len();
                write!(&mut picker.symbol_corpus.bytes, "{line_number}: ").unwrap();
                buffer_append_range(
                    &document.buffer,
                    line_start,
                    line_end,
                    &mut picker.symbol_corpus.bytes,
                );
                reference_target_push(
                    picker,
                    u32::MAX,
                    position,
                    position + name.len(),
                    label_start,
                );
            }
            position += 1;
        }
        if line_end >= length {
            break;
        }
        line_start = line_end + 1;
        line_number += 1;
    }
    if !workspace {
        return;
    }
    let Some(corpus) = editor.project_search.as_ref() else {
        return;
    };
    for line in &corpus.lines {
        let project_file = line.project_file as usize;
        if current_project_file == Some(project_file) {
            continue;
        }
        let source = &corpus.bytes[line.text_start as usize..line.display_end as usize];
        let mut position = 0;
        while position + name.len() <= source.len() {
            if slice_range_matches_identifier(source, position, name) {
                let label_start = picker.symbol_corpus.bytes.len();
                picker.symbol_corpus.bytes.extend_from_slice(
                    &corpus.bytes[line.display_start as usize..line.display_end as usize],
                );
                reference_target_push(
                    picker,
                    line.project_file,
                    line.file_offset as usize + position,
                    line.file_offset as usize + position + name.len(),
                    label_start,
                );
            }
            position += 1;
        }
    }
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

fn slice_range_matches_identifier(source: &[u8], start: usize, name: &[u8]) -> bool {
    let end = start + name.len();
    (start == 0 || !rust_identifier_byte(source[start - 1]))
        && (end == source.len() || !rust_identifier_byte(source[end]))
        && source.get(start..end) == Some(name)
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
    let after = editor_location(editor);
    editor_record_jump(editor, before, after);
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
    let force = width != editor.terminal_width
        || height != editor.terminal_height
        || picker != editor.rendered_picker
        || editor.theme != editor.rendered_theme;
    editor.terminal_width = width;
    editor.terminal_height = height;
    editor.rendered_picker = picker;
    editor.rendered_theme = editor.theme;
    if picker {
        editor.rendered_row_hashes.clear();
        editor.rendered_overlay_start = None;
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
    let (cursor_line, cursor_column) = buffer_line_and_column(&document.buffer, primary_position);
    if cursor_line < document.top_line + scroll_margin {
        document.top_line = cursor_line.saturating_sub(scroll_margin);
    } else if cursor_line + scroll_margin >= document.top_line + content_height {
        document.top_line = cursor_line + scroll_margin + 1 - content_height;
    }
    let top_line = document.top_line;
    let total_lines = buffer_line_count(&document.buffer);
    let number_width = (total_lines.max(1).ilog10() as usize + 1).max(5);
    let gutter_width = (number_width + 1).min(width);
    let text_width = width.saturating_sub(gutter_width);
    let temp = idno_std::mem().scratch().temp();
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
        .extend_from_slice(b"\x1b[?2026h\x1b[?25l\x1b[H");
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
        if line < total_lines && !(trailing_empty_line && line + 1 == total_lines) {
            write!(&mut editor.frame, "{:>number_width$}", line + 1).unwrap();
        } else {
            write!(&mut editor.frame, "{:>number_width$}", "~").unwrap();
        }
        let git_flags = git_gutter_flags(&document.git_gutter, line);
        if git_gutter_line_removed(git_flags) {
            editor.frame.extend_from_slice(theme.git_removed);
            editor.frame.extend_from_slice("╴".as_bytes());
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
                && selections.iter().any(|selection| {
                    (mode != Mode::Insert || selection.anchor != selection.cursor)
                        && position_is_selected(
                            &document.buffer,
                            std::slice::from_ref(selection),
                            position,
                        )
                });
            let position_search_scope = search.is_some_and(|search| {
                search.kind == SearchKind::Selection
                    && position_is_selected(&document.buffer, &search.original_selections, position)
            });
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
                position_is_selected(&document.buffer, &search.matches, position)
            });
            let wanted_style = if secondary_insert_cursor || normal_cursor || search_cursor {
                3
            } else if position_selected || position_search_match {
                2
            } else if position_search_scope {
                1
            } else {
                syntax_style.unwrap_or(0)
            };
            if wanted_style != rendered_style {
                let style = match wanted_style {
                    3 => theme.cursor,
                    2 => theme.selection,
                    1 => theme.search_scope,
                    4.. => theme.syntax[(wanted_style - 4) as usize],
                    _ => theme.normal,
                };
                editor.frame.extend_from_slice(style);
                rendered_style = wanted_style;
            }
            let byte = buffer_byte(&document.buffer, position);
            match byte {
                b'\t' => {
                    let spaces = (4 - column % 4).min(text_width - column);
                    editor.frame.extend(std::iter::repeat_n(b' ', spaces));
                    column += spaces;
                }
                0x00..=0x1f | 0x7f => {
                    editor.frame.push(b'?');
                    column += 1;
                }
                _ => {
                    editor.frame.push(byte);
                    if byte & 0b1100_0000 != 0b1000_0000 {
                        column += 1;
                    }
                }
            }
            position += 1;
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
            let position_selected =
                search.is_none() && position_is_selected(&document.buffer, &selections, position);
            let position_search_scope = search.is_some_and(|search| {
                search.kind == SearchKind::Selection
                    && position_is_selected(&document.buffer, &search.original_selections, position)
            });
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
                position_is_selected(&document.buffer, &search.matches, position)
            });
            if column < text_width {
                let style = if secondary_insert_cursor || normal_cursor || search_cursor {
                    theme.cursor
                } else if position_selected || position_search_match {
                    theme.selection
                } else if position_search_scope {
                    theme.search_scope
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
    if key_hints.is_empty()
        && let Some(completion) = editor.completion.as_ref()
        && width > 0
        && content_height > 0
    {
        let visible = completion.matches.len().min(8).min(content_height);
        let first = completion
            .selected
            .saturating_sub(visible.saturating_sub(1));
        let popup_width = completion.matches[first..first + visible]
            .iter()
            .map(|found| rust_method_name(&editor.rust_methods.corpus, found.item).len() + 2)
            .max()
            .unwrap_or(0)
            .min(width);
        let popup_column = width.saturating_sub(popup_width) + 1;
        let popup_row = content_height.saturating_sub(visible) + 1;
        for (row, found) in completion.matches[first..first + visible]
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
            let name = rust_method_name(&editor.rust_methods.corpus, found.item);
            write!(
                &mut editor.frame,
                " {:<name_width$} ",
                name,
                name_width = popup_width.saturating_sub(2)
            )
            .unwrap();
        }
        editor.frame.extend_from_slice(theme.normal);
    }
    let screen_row = cursor_line.saturating_sub(top_line) + 1;
    let screen_column = (gutter_width + cursor_column).min(width.saturating_sub(1)) + 1;
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
        editor.completion.as_ref().map(|completion| {
            content_height.saturating_sub(completion.matches.len().min(8).min(content_height))
        })
    };
    editor.frame.extend_from_slice(b"\x1b[?2026l");
    editor_present_document_rows(
        editor,
        terminal,
        &row_ranges,
        status_start,
        overlay_start,
        force,
    )
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
                .push(render_bytes_hash(&editor.frame[range.clone()]));
        }
        return terminal::terminal_present(terminal, &editor.frame);
    }
    editor.present_frame.clear();
    editor
        .present_frame
        .extend_from_slice(b"\x1b[?2026h\x1b[?25l");
    for (row, range) in row_ranges.iter().enumerate() {
        let hash = render_bytes_hash(&editor.frame[range.clone()]);
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

fn render_bytes_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &byte in bytes {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
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
    editor.frame.clear();
    editor
        .frame
        .extend_from_slice(b"\x1b[?2026h\x1b[?25l\x1b[H");
    editor.frame.extend_from_slice(theme.cursor_color);
    editor.frame.extend_from_slice(theme.normal);
    let match_count = if picker.kind == PickerKind::Files && picker.query.is_empty() {
        editor.project.labels.len()
    } else {
        picker.matches.len()
    };
    write!(
        &mut editor.frame,
        " {title}  {} matches{}",
        match_count,
        if picker.kind == PickerKind::Files && editor.project_discovery.is_some() {
            "  scanning"
        } else if picker.kind == PickerKind::SearchProject && !picker.search_complete {
            "  searching"
        } else {
            ""
        }
    )
    .unwrap();
    editor.frame.extend_from_slice(b"\x1b[K\r\n");
    let result_rows = height.saturating_sub(2);
    let first = picker
        .selected
        .saturating_sub(result_rows.saturating_sub(1));
    for visible in 0..result_rows {
        let match_position = first + visible;
        let item = if picker.kind == PickerKind::Files && picker.query.is_empty() {
            (match_position < editor.project.labels.len()).then_some(match_position)
        } else if picker.kind == PickerKind::Files {
            picker
                .search_ranked
                .get(match_position)
                .map(|found| found.item)
        } else if picker.kind == PickerKind::SearchProject {
            picker
                .search_ranked
                .get(match_position)
                .map(|found| found.item)
        } else {
            picker.matches.get(match_position).map(|found| found.item)
        };
        if let Some(item) = item {
            editor
                .frame
                .extend_from_slice(if match_position == picker.selected {
                    theme.picker_selected
                } else {
                    theme.normal
                });
            editor
                .frame
                .extend_from_slice(if match_position == picker.selected {
                    b"> "
                } else {
                    b"  "
                });
            let label = match picker.kind {
                PickerKind::Files => editor.project.labels[item].as_str(),
                PickerKind::Commands => {
                    if command_theme_argument(&picker.query).is_some() {
                        THEME_NAMES[item]
                    } else if command_file_argument(&picker.query).is_some() {
                        editor.project.labels[item].as_str()
                    } else {
                        COMMANDS[item]
                    }
                }
                PickerKind::SearchProject => {
                    let Some(corpus) = editor.project_search.as_ref() else {
                        continue;
                    };
                    let line = corpus.lines[item];
                    std::str::from_utf8(
                        &corpus.bytes[line.display_start as usize..line.display_end as usize],
                    )
                    .unwrap_or("")
                }
                PickerKind::DocumentSymbols => {
                    let line = picker.symbol_corpus.lines[picker.symbol_candidates[item]];
                    std::str::from_utf8(
                        &picker.symbol_corpus.bytes
                            [line.display_start as usize..line.display_end as usize],
                    )
                    .unwrap_or("")
                }
                PickerKind::WorkspaceSymbols => {
                    let Some(&symbol) = picker.rust_symbol_candidates.get(item) else {
                        continue;
                    };
                    rust_symbol_name(&editor.rust_methods.corpus, symbol)
                }
                PickerKind::References => {
                    let line = picker.symbol_corpus.lines[item];
                    std::str::from_utf8(
                        &picker.symbol_corpus.bytes
                            [line.display_start as usize..line.display_end as usize],
                    )
                    .unwrap_or("")
                }
                PickerKind::DocumentDiagnostics | PickerKind::WorkspaceDiagnostics => {
                    let diagnostic = picker.diagnostic_candidates[item];
                    editor.diagnostics.published[diagnostic].display.as_str()
                }
            };
            let mut end = label.len().min(width.saturating_sub(2));
            while !label.is_char_boundary(end) {
                end -= 1;
            }
            editor.frame.extend_from_slice(&label.as_bytes()[..end]);
        }
        editor.frame.extend_from_slice(theme.normal);
        editor.frame.extend_from_slice(b"\x1b[K");
        if visible + 1 < result_rows {
            editor.frame.extend_from_slice(b"\r\n");
        }
    }
    write!(&mut editor.frame, "\x1b[{};1H", height).unwrap();
    editor.frame.extend_from_slice(theme.status);
    write!(&mut editor.frame, "> {}\x1b[K", picker.query).unwrap();
    editor.frame.extend_from_slice(theme.normal);
    let query_column = (picker.query.chars().count() + 3).min(width);
    write!(
        &mut editor.frame,
        "\x1b[{};{}H\x1b[6 q\x1b[?25h",
        height, query_column
    )
    .unwrap();
    editor.frame.extend_from_slice(b"\x1b[?2026l");
    terminal::terminal_present(terminal, &editor.frame)
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
    fn word_motions_create_and_extend_a_selection() {
        let mut editor = editor_with_text("one two three");
        editor_handle_key(&mut editor, Key::Character('w'));
        assert_eq!(editor.mode, Mode::Normal);
        assert_eq!(editor_document(&editor).anchor, 0);
        assert_eq!(editor_document(&editor).cursor, 4);
        editor_handle_key(&mut editor, Key::Character('w'));
        assert_eq!(editor_document(&editor).cursor, 8);
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
        std::fs::create_dir(&directory).unwrap();
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
    fn goto_module_definition_opens_the_module_file() {
        static NEXT_DIRECTORY: std::sync::atomic::AtomicUsize =
            std::sync::atomic::AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "bed-module-definition-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).unwrap();
        let main = directory.join("main.rs");
        let module = directory.join("potato.rs");
        std::fs::write(&main, b"mod potato;\nfn main() { grow(); }").unwrap();
        std::fs::write(&module, b"pub fn grow() {}").unwrap();
        let mut editor = editor_open(Some(main)).unwrap();
        editor.documents[0].cursor = 4;
        editor.documents[0].anchor = 4;
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

        editor_switch_document(&mut editor, 0);
        editor.documents[0].cursor = 24;
        editor.documents[0].anchor = 24;
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
        assert_eq!(editor_document(&editor).cursor, 24);

        std::fs::remove_dir_all(directory).unwrap();
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
    fn enter_copies_indentation_and_indents_open_scopes() {
        let mut editor = editor_with_text("    {");
        editor_handle_key(&mut editor, Key::Character('A'));
        editor_handle_key(&mut editor, Key::Enter);
        assert_eq!(contents(&editor), b"    {\n        ");
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
    fn explicit_type_member_completion_accepts_with_tab() {
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
        assert_eq!(contents(&editor), b"let potato: Vec<usize>; potato.push");
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
        };
        for item in 0..5000 {
            let start = corpus.bytes.len();
            write!(&mut corpus.bytes, "needle item {item}").unwrap();
            let end = corpus.bytes.len();
            corpus.lines.push(SearchLine {
                project_file: 0,
                file_offset: 0,
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
