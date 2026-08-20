use core::alloc::Allocator;
use std::fmt::Write as _;
use std::io::Read as _;
use std::io::Write as _;

use crate::buffer::*;
use crate::fuzzy::{FuzzyMatch, fuzzy_byte_matches, fuzzy_rank};
use crate::project::{
    ProjectDiscoveryState, ProjectFiles, project_discover, project_discovery_step,
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
}

bitfield::bitfield! {
    pub struct EditorFlags: 1 {
        const AUTO_INDENTATION = 0;
        const AUTO_INDENT_SCOPES = 1;
    }
}

pub struct EditorConfig {
    pub flags: EditorFlags,
    pub indentation_spaces: usize,
    pub scroll_margin_lines: usize,
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
    pub last_accessed_document: Option<usize>,
    pub config: EditorConfig,
    pub viewport_height: usize,
    pub jumps: Vec<Jump>,
    pub jump_position: usize,
    pub quit_warning: bool,
    pub quit_requested: bool,
    pub status: String,
    pub frame: Vec<u8>,
    pub theme: usize,
    pub search_query: String,
    pub search_matches: Vec<SelectionState>,
    pub search_position: usize,
    pub search: Option<SearchSession>,
    pub project_search: Option<SearchCorpus>,
    pub project_search_task: Option<idno_std::micropool::OwnedTask<SearchCorpus>>,
    pub project_discovery: Option<ProjectDiscoveryState>,
}

const COMMANDS: [&str; 57] = [
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
    "toggle-auto-indentation",
    "toggle-auto-indent-scopes",
];

pub struct Theme {
    pub name: &'static str,
    pub normal: &'static [u8],
    pub gutter: &'static [u8],
    pub status: &'static [u8],
    pub search_scope: &'static [u8],
    pub selection: &'static [u8],
    pub cursor: &'static [u8],
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
        picker_selected: b"\x1b[38;2;38;36;48m\x1b[48;2;205;145;165m",
        cursor_color: b"\x1b]12;#cd91a5\x07",
    },
];

pub fn editor_open(path: Option<std::path::PathBuf>) -> std::io::Result<Editor> {
    let (root, initial_file, open_picker) = match path {
        Some(path) if path.is_dir() => (path, None, true),
        Some(path) => {
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
        last_accessed_document: None,
        config: EditorConfig {
            flags: EditorFlags::AUTO_INDENTATION | EditorFlags::AUTO_INDENT_SCOPES,
            indentation_spaces: 4,
            scroll_margin_lines: 3,
        },
        viewport_height: 23,
        jumps: Vec::new(),
        jump_position: 0,
        quit_warning: false,
        quit_requested: false,
        status: ignore_status.unwrap_or_default(),
        frame: Vec::with_capacity(32 * 1024),
        theme: 0,
        search_query: String::new(),
        search_matches: Vec::new(),
        search_position: 0,
        search: None,
        project_search,
        project_search_task,
        project_discovery,
    };
    if open_picker {
        editor_open_picker(&mut editor, PickerKind::Files);
    }
    Ok(editor)
}

pub fn document_empty() -> Document {
    Document {
        buffer: buffer_from_bytes(&[]),
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
            | editor_step_project_search(editor);
        if input_changed || background_changed {
            match editor_render(editor, terminal) {
                Ok(()) => {}
                Err(error) => return Err(error),
            }
        }
        let key = if editor_background_work_pending(editor) {
            match terminal::terminal_read_key_timeout(terminal, 16) {
                Ok(Some(key)) => key,
                Ok(None) => {
                    input_changed = false;
                    continue;
                }
                Err(error) => return Err(error),
            }
        } else {
            match terminal::terminal_read_key(terminal) {
                Ok(key) => key,
                Err(error) => return Err(error),
            }
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
        editor.pending_key = PendingKey::None;
        match (pending, key) {
            (PendingKey::Space, Key::Character('f')) => {
                editor_open_picker(editor, PickerKind::Files)
            }
            (PendingKey::Space, Key::Character('t')) => editor.pending_key = PendingKey::SpaceTheme,
            (PendingKey::Space, Key::Character('/')) => {
                editor_open_picker(editor, PickerKind::SearchProject)
            }
            (PendingKey::SpaceTheme, Key::Character('d' | 'k')) => editor_set_theme(editor, 0),
            (PendingKey::SpaceTheme, Key::Character('b')) => editor_set_theme(editor, 1),
            (PendingKey::SpaceTheme, Key::Character('l')) => editor_set_theme(editor, 2),
            (PendingKey::SpaceTheme, Key::Character('n')) => editor_set_theme(editor, 3),
            (PendingKey::SpaceTheme, Key::Character('v')) => editor_set_theme(editor, 4),
            (PendingKey::Goto, Key::Character('g')) => editor_goto_file_start(editor),
            (PendingKey::Goto, Key::Character('e')) => editor_goto_last_line(editor),
            (PendingKey::Goto, Key::Character('h')) => editor_move_line_boundary(editor, false),
            (PendingKey::Goto, Key::Character('l')) => editor_move_line_boundary(editor, true),
            (PendingKey::Goto, Key::Character('s')) => editor_goto_first_nonwhitespace(editor),
            (PendingKey::Goto, Key::Character('a')) => editor_goto_last_accessed_document(editor),
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
                editor_select_surrounding(editor, delimiter, false)
            }
            (PendingKey::MatchInside, Key::Character(delimiter)) => {
                editor_select_surrounding(editor, delimiter, true)
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
    true
}

pub fn document_undo(document: &mut Document) {
    profiling::function_scope!();
    let Some(transaction) = document.undo.pop() else {
        return;
    };
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
    let mut before = Vec::with_capacity(document.secondary_selections.len() + 1);
    document_selections(document, &mut before);
    let retained_insert_selection = document.active_transaction.is_some()
        && !document.insertion_points.is_empty()
        && requested_after.is_none();
    let temp = idno_std::mem().scratch().temp();
    let mut retained_selection_bounds = temp.vec(before.len());
    let mut transformed_insertion_points = temp.vec(document.insertion_points.len());
    if retained_insert_selection {
        for selection in &before {
            retained_selection_bounds.push((
                selection.anchor.min(selection.cursor),
                buffer_next_char(&document.buffer, selection.anchor.max(selection.cursor)),
                selection.anchor <= selection.cursor,
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
        for &(start, end, forward) in &retained_selection_bounds {
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
    };
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
        PickerKind::Files | PickerKind::SearchProject => {}
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
        || editor.picker.as_ref().is_some_and(|picker| {
            picker.kind == PickerKind::SearchProject && !picker.search_complete
        })
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
    let text = &corpus.bytes[line.text_start as usize..line.display_end as usize];
    let span = fuzzy_subsequence_span(picker.query.as_bytes(), text);
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
    let (start, end) = match span {
        Some((start, end)) => (
            (file_offset + start).min(line_end),
            (file_offset + end).min(line_end),
        ),
        None => (file_offset.min(line_end), file_offset.min(line_end)),
    };
    document.anchor = start;
    document.cursor = if end > start {
        buffer_previous_char(&document.buffer, end)
    } else {
        start
    };
    document.secondary_selections.clear();
    document.preferred_column = buffer_line_and_column(&document.buffer, document.cursor).1;
    editor.search_position = editor
        .search_matches
        .iter()
        .position(|selection| selection.anchor == start)
        .unwrap_or(0);
    editor.mode = Mode::Normal;
}

fn editor_start_search(editor: &mut Editor, kind: SearchKind) {
    profiling::function_scope!();
    let document = editor_document(editor);
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
            search_document_fuzzy_matches(search);
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

fn search_document_fuzzy_matches(search: &mut SearchSession) {
    profiling::function_scope!();
    let temp = idno_std::mem().scratch().temp();
    let mut labels = temp.vec(search.corpus.lines.len());
    for line in &search.corpus.lines {
        let text = &search.corpus.bytes[line.text_start as usize..line.display_end as usize];
        labels.push(std::str::from_utf8(text).unwrap_or(""));
    }
    let mut ranked = temp.vec(search.corpus.lines.len());
    fuzzy_rank(&search.query, &labels, &mut ranked);
    search.matches.reserve(ranked.len());
    for found in &ranked {
        let line = search.corpus.lines[found.item];
        let text = &search.corpus.bytes[line.text_start as usize..line.display_end as usize];
        if let Some((start, end)) = fuzzy_subsequence_span(search.query.as_bytes(), text) {
            let file_offset = line.file_offset as usize;
            search.matches.push(SelectionState {
                anchor: file_offset + start,
                cursor: file_offset + end.saturating_sub(1),
            });
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
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    let document_index = editor.documents.iter().position(|document| {
        document.path.as_ref().is_some_and(|open| {
            std::fs::canonicalize(open).unwrap_or_else(|_| open.clone()) == path
        })
    });
    let target = match document_index {
        Some(document) => document,
        None => match document_open(Some(path)) {
            Ok(document) => {
                editor.documents.push(document);
                editor.documents.len() - 1
            }
            Err(error) => {
                write!(&mut editor.status, "open failed: {error}").unwrap();
                return;
            }
        },
    };
    editor_switch_document(editor, target);
}

pub fn editor_switch_document(editor: &mut Editor, target: usize) {
    if target == editor.current || target >= editor.documents.len() {
        return;
    }
    document_commit_transaction(editor_document_mut(editor));
    let before = editor_location(editor);
    editor.last_accessed_document = Some(editor.current);
    editor.current = target;
    editor.mode = Mode::Normal;
    let after = editor_location(editor);
    editor_record_jump(editor, before, after);
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
        _ => write!(&mut editor.status, "unknown command: {command}").unwrap(),
    }
}

fn editor_set_document_path(editor: &mut Editor, path: &str) {
    let path = std::path::Path::new(path);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        editor.project.root.join(path)
    };
    editor_document_mut(editor).path = Some(path);
}

fn editor_open_path(editor: &mut Editor, path: &str) {
    profiling::function_scope!();
    let path = std::path::Path::new(path);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        editor.project.root.join(path)
    };
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    if let Some(target) = editor.documents.iter().position(|document| {
        document.path.as_ref().is_some_and(|open| {
            std::fs::canonicalize(open).unwrap_or_else(|_| open.clone()) == path
        })
    }) {
        editor_switch_document(editor, target);
        return;
    }
    match document_open(Some(path)) {
        Ok(document) => {
            editor.documents.push(document);
            editor_switch_document(editor, editor.documents.len() - 1);
        }
        Err(error) => write!(&mut editor.status, "open failed: {error}").unwrap(),
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
            editor.mode = Mode::Normal;
            let document = editor_document_mut(editor);
            document.insertion_points.clear();
            document_commit_transaction(document);
        }
        Key::Backspace => {
            let document = editor_document_mut(editor);
            let temp = idno_std::mem().scratch().temp();
            let mut replacements = temp.vec(document.insertion_points.len());
            for &position in &document.insertion_points {
                if position > 0 {
                    let previous = buffer_previous_char(&document.buffer, position);
                    replacements.push(Replacement {
                        start: previous,
                        end: position,
                        inserted: 0..0,
                    });
                }
            }
            document_replace_ranges(document, &mut replacements, &[], None);
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
        Key::Enter => editor_insert_newline(editor),
        Key::Tab => editor_insert(editor, "\t"),
        Key::Character(character) => {
            let mut encoded = [0; 4];
            editor_insert(editor, character.encode_utf8(&mut encoded));
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

fn editor_handle_command_key(editor: &mut Editor, key: Key) -> bool {
    match key {
        Key::Character(':') => editor_open_picker(editor, PickerKind::Commands),
        Key::Character('/') => editor_start_search(editor, SearchKind::Document),
        Key::Character('s') => editor_start_search(editor, SearchKind::Selection),
        Key::Character(' ') if editor.mode == Mode::Normal => {
            editor.pending_key = PendingKey::Space
        }
        Key::Character('m') => editor.pending_key = PendingKey::Match,
        Key::Character('g') => editor.pending_key = PendingKey::Goto,
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
    document_replace_ranges(document, &mut replacements, &inserted_bytes, None);
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
    document_replace_ranges(document, &mut replacements, &inserted_bytes, None);
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
    document_replace_ranges(document, &mut replacements, &inserted_bytes, None);
    for position in &mut document.insertion_points {
        *position = position.saturating_sub(1);
    }
    editor.mode = Mode::Insert;
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
    editor.mode = Mode::Select;
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
    let (line, column) = buffer_line_and_column(&document.buffer, document.cursor);
    if document.preferred_column == 0 || column != 0 {
        document.preferred_column = column;
    }
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
    if editor.picker.is_some() {
        return editor_render_picker(editor, terminal, width, height);
    }
    editor_render_document(editor, terminal, width, height)
}

fn editor_render_document(
    editor: &mut Editor,
    terminal: &mut Terminal,
    width: usize,
    height: usize,
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
    document_selections(document, &mut selections);
    let path = document
        .path
        .as_deref()
        .map(|path| path.strip_prefix(project_root).unwrap_or(path));
    let dirty = document.modified;

    editor.frame.clear();
    editor.frame.extend_from_slice(b"\x1b[?25l\x1b[H");
    editor.frame.extend_from_slice(theme.cursor_color);
    editor.frame.extend_from_slice(theme.normal);
    editor.frame.extend_from_slice(b"\x1b[2J");
    let mut position = buffer_position_at_line_column(&document.buffer, top_line, 0);
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
        let line = top_line + row;
        editor.frame.extend_from_slice(theme.gutter);
        if line < total_lines && !(trailing_empty_line && line + 1 == total_lines) {
            write!(&mut editor.frame, "{:>number_width$} ", line + 1).unwrap();
        } else {
            write!(&mut editor.frame, "{:>number_width$} ", "~").unwrap();
        }
        editor.frame.extend_from_slice(theme.normal);
        let mut column = 0;
        while position < length
            && buffer_byte(&document.buffer, position) != b'\n'
            && column < text_width
        {
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
            let wanted_style = if secondary_insert_cursor || normal_cursor || search_cursor {
                3
            } else if position_selected || position_search_match {
                2
            } else if position_search_scope {
                1
            } else {
                0
            };
            if wanted_style != rendered_style {
                editor.frame.extend_from_slice(match wanted_style {
                    3 => theme.cursor,
                    2 => theme.selection,
                    1 => theme.search_scope,
                    _ => theme.normal,
                });
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
        rendered_style = 0;
        if row + 1 < content_height {
            editor.frame.extend_from_slice(b"\r\n");
        }
    }
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
        if editor.pending_key != PendingKey::None {
            write!(&mut editor.frame, " {:?}", editor.pending_key).unwrap();
        }
        if !editor.status.is_empty() {
            write!(&mut editor.frame, " {}", editor.status).unwrap();
        }
    }
    editor.frame.extend_from_slice(b"\x1b[K");
    editor.frame.extend_from_slice(theme.normal);
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
    terminal::terminal_present(terminal, &editor.frame)
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
    };
    editor.frame.clear();
    editor.frame.extend_from_slice(b"\x1b[?25l\x1b[H");
    editor.frame.extend_from_slice(theme.cursor_color);
    editor.frame.extend_from_slice(theme.normal);
    editor.frame.extend_from_slice(b"\x1b[2J");
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
            (0, 4)
        );
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
    fn enter_copies_indentation_and_indents_open_scopes() {
        let mut editor = editor_with_text("    {");
        editor_handle_key(&mut editor, Key::Character('A'));
        editor_handle_key(&mut editor, Key::Enter);
        assert_eq!(contents(&editor), b"    {\n        ");
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
    fn fuzzy_search_in_document_selects_the_matching_span() {
        let mut editor = editor_with_text("alpha\nimportant needle here\nomega\n");
        editor_handle_key(&mut editor, Key::Character('/'));
        for character in "nede".chars() {
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
    fn document_search_stays_in_place_and_escape_cancels() {
        let mut editor = editor_with_text("one needle two");
        editor.documents[0].cursor = 4;
        editor.documents[0].anchor = 4;
        editor_handle_key(&mut editor, Key::Character('/'));
        assert!(editor.picker.is_none());
        assert!(editor.search.is_some());
        for character in "nedl".chars() {
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
}
