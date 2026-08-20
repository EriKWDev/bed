use crate::buffer::{GapBuffer, buffer_byte, buffer_len};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeSymbolKind {
    Value,
    Function,
    Type,
    Module,
    Constant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodeIdentifier {
    pub start: u32,
    pub end: u32,
    pub scope_depth: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodeSymbol {
    pub identifier: u32,
    pub kind: CodeSymbolKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeIndexMode {
    Code,
    LineComment,
    BlockComment,
    String,
}

pub struct CodeIndex {
    pub identifiers: Vec<CodeIdentifier>,
    pub symbols: Vec<CodeSymbol>,
    pub checkpoints: Vec<CodeIndexCheckpoint>,
    pub position: usize,
    pub scope_depth: u16,
    pub block_depth: u16,
    pub mode: CodeIndexMode,
    pub delimiter: u8,
    pub escaped: bool,
    pub expected_symbol: Option<CodeSymbolKind>,
    pub enabled: bool,
    pub complete: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodeIndexCheckpoint {
    pub position: u32,
    pub identifier_count: u32,
    pub symbol_count: u32,
    pub scope_depth: u16,
    pub block_depth: u16,
    pub mode: CodeIndexMode,
    pub delimiter: u8,
    pub escaped: bool,
    pub expected_symbol: Option<CodeSymbolKind>,
}

pub fn code_index_empty() -> CodeIndex {
    CodeIndex {
        identifiers: Vec::new(),
        symbols: Vec::new(),
        checkpoints: Vec::new(),
        position: 0,
        scope_depth: 0,
        block_depth: 0,
        mode: CodeIndexMode::Code,
        delimiter: 0,
        escaped: false,
        expected_symbol: None,
        enabled: false,
        complete: true,
    }
}

pub fn code_index_set_path(index: &mut CodeIndex, path: Option<&std::path::Path>) {
    profiling::function_scope!();
    index.enabled = path
        .and_then(std::path::Path::extension)
        .and_then(std::ffi::OsStr::to_str)
        == Some("rs");
    code_index_invalidate(index);
}

pub fn code_index_invalidate(index: &mut CodeIndex) {
    profiling::function_scope!();
    let CodeIndex {
        identifiers,
        symbols,
        checkpoints,
        position,
        scope_depth,
        block_depth,
        mode,
        delimiter,
        escaped,
        expected_symbol,
        enabled,
        complete,
    } = index;
    identifiers.clear();
    symbols.clear();
    checkpoints.clear();
    *position = 0;
    *scope_depth = 0;
    *block_depth = 0;
    *mode = CodeIndexMode::Code;
    *delimiter = 0;
    *escaped = false;
    *expected_symbol = None;
    *complete = !*enabled;
    checkpoints.push(CodeIndexCheckpoint {
        position: 0,
        identifier_count: 0,
        symbol_count: 0,
        scope_depth: 0,
        block_depth: 0,
        mode: CodeIndexMode::Code,
        delimiter: 0,
        escaped: false,
        expected_symbol: None,
    });
}

pub fn code_index_invalidate_edits(index: &mut CodeIndex, edits: &[(usize, usize, usize)]) {
    profiling::function_scope!();
    if edits.is_empty() || !index.enabled {
        return;
    }
    let first_edit = edits.iter().map(|edit| edit.0).min().unwrap_or(0);
    let checkpoint_index = index
        .checkpoints
        .partition_point(|checkpoint| checkpoint.position as usize <= first_edit)
        .saturating_sub(1);
    let checkpoint =
        index
            .checkpoints
            .get(checkpoint_index)
            .copied()
            .unwrap_or(CodeIndexCheckpoint {
                position: 0,
                identifier_count: 0,
                symbol_count: 0,
                scope_depth: 0,
                block_depth: 0,
                mode: CodeIndexMode::Code,
                delimiter: 0,
                escaped: false,
                expected_symbol: None,
            });
    index
        .identifiers
        .truncate(checkpoint.identifier_count as usize);
    index.symbols.truncate(checkpoint.symbol_count as usize);
    index.checkpoints.truncate(checkpoint_index + 1);
    index.position = checkpoint.position as usize;
    index.scope_depth = checkpoint.scope_depth;
    index.block_depth = checkpoint.block_depth;
    index.mode = checkpoint.mode;
    index.delimiter = checkpoint.delimiter;
    index.escaped = checkpoint.escaped;
    index.expected_symbol = checkpoint.expected_symbol;
    index.complete = false;
}

pub fn code_index_step(
    buffer: &GapBuffer,
    index: &mut CodeIndex,
    maximum_bytes: usize,
    maximum_time: std::time::Duration,
) -> bool {
    profiling::function_scope!();
    if index.complete || !index.enabled {
        return false;
    }
    let original_identifiers = index.identifiers.len();
    let length = buffer_len(buffer).min(u32::MAX as usize);
    let end = index.position.saturating_add(maximum_bytes).min(length);
    let started = std::time::Instant::now();
    profiling::scope!("scan rust symbols");
    while index.position < end {
        if index.position & 255 == 0 && started.elapsed() >= maximum_time {
            break;
        }
        let previous_position = index.position;
        match index.mode {
            CodeIndexMode::Code => code_index_scan_code(buffer, index, length),
            CodeIndexMode::LineComment => {
                if buffer_byte(buffer, index.position) == b'\n' {
                    index.mode = CodeIndexMode::Code;
                } else {
                    index.position += 1;
                }
            }
            CodeIndexMode::BlockComment => code_index_scan_block_comment(buffer, index, length),
            CodeIndexMode::String => code_index_scan_string(buffer, index),
        }
        if index.position > previous_position && buffer_byte(buffer, index.position - 1) == b'\n' {
            code_index_checkpoint_push(index);
        }
    }
    if index.position >= length {
        index.complete = true;
    }
    index.identifiers.len() != original_identifiers
}

fn code_index_checkpoint_push(index: &mut CodeIndex) {
    if index.position > u32::MAX as usize
        || index.identifiers.len() > u32::MAX as usize
        || index.symbols.len() > u32::MAX as usize
    {
        return;
    }
    let checkpoint = CodeIndexCheckpoint {
        position: index.position as u32,
        identifier_count: index.identifiers.len() as u32,
        symbol_count: index.symbols.len() as u32,
        scope_depth: index.scope_depth,
        block_depth: index.block_depth,
        mode: index.mode,
        delimiter: index.delimiter,
        escaped: index.escaped,
        expected_symbol: index.expected_symbol,
    };
    if index
        .checkpoints
        .last()
        .is_none_or(|previous| previous.position < checkpoint.position)
    {
        index.checkpoints.push(checkpoint);
    }
}

fn code_index_scan_code(buffer: &GapBuffer, index: &mut CodeIndex, length: usize) {
    let byte = buffer_byte(buffer, index.position);
    let next = (index.position + 1 < length).then(|| buffer_byte(buffer, index.position + 1));
    if byte == b'/' && next == Some(b'/') {
        index.mode = CodeIndexMode::LineComment;
        index.position += 2;
    } else if byte == b'/' && next == Some(b'*') {
        index.mode = CodeIndexMode::BlockComment;
        index.block_depth = 1;
        index.position += 2;
    } else if byte == b'\'' {
        let lifetime_end = code_rust_lifetime_end(buffer, index.position, length);
        if lifetime_end > index.position {
            index.position = lifetime_end;
        } else {
            index.mode = CodeIndexMode::String;
            index.delimiter = byte;
            index.escaped = false;
            index.position += 1;
        }
    } else if byte == b'"' {
        index.mode = CodeIndexMode::String;
        index.delimiter = byte;
        index.escaped = false;
        index.position += 1;
    } else if byte == b'{' {
        index.scope_depth = index.scope_depth.saturating_add(1);
        index.position += 1;
    } else if byte == b'}' {
        index.scope_depth = index.scope_depth.saturating_sub(1);
        index.position += 1;
    } else if code_identifier_byte(byte) && !byte.is_ascii_digit() {
        let start = index.position;
        index.position += 1;
        while index.position < length && code_identifier_byte(buffer_byte(buffer, index.position)) {
            index.position += 1;
        }
        code_index_identifier(buffer, index, start, index.position, length);
    } else {
        index.position += 1;
    }
}

fn code_index_identifier(
    buffer: &GapBuffer,
    index: &mut CodeIndex,
    start: usize,
    end: usize,
    length: usize,
) {
    if let Some(symbol_kind) = code_definition_keyword(buffer, start, end) {
        index.expected_symbol = Some(symbol_kind);
        return;
    }
    if code_rust_keyword(buffer, start, end) {
        if !code_range_equal(buffer, start, end, b"mut")
            && !code_range_equal(buffer, start, end, b"pub")
        {
            index.expected_symbol = None;
        }
        return;
    }
    let identifier = index.identifiers.len();
    if identifier > u32::MAX as usize || end > u32::MAX as usize {
        return;
    }
    index.identifiers.push(CodeIdentifier {
        start: start as u32,
        end: end as u32,
        scope_depth: index.scope_depth,
    });
    let followed_by_field_colon = code_next_nonwhitespace(buffer, end, length) == Some(b':')
        && !code_bytes_equal(buffer, end, b"::");
    if index.expected_symbol == Some(CodeSymbolKind::Value)
        && buffer_byte(buffer, start).is_ascii_uppercase()
        && code_next_nonwhitespace(buffer, end, length) == Some(b'(')
    {
        return;
    }
    let kind = index.expected_symbol.take().or_else(|| {
        (code_identifier_is_match_binding(buffer, start, end, length) || followed_by_field_colon)
            .then_some(CodeSymbolKind::Value)
    });
    if let Some(kind) = kind {
        index.symbols.push(CodeSymbol {
            identifier: identifier as u32,
            kind,
        });
    }
}

fn code_identifier_is_match_binding(
    buffer: &GapBuffer,
    start: usize,
    end: usize,
    length: usize,
) -> bool {
    if !buffer_byte(buffer, start).is_ascii_lowercase() && buffer_byte(buffer, start) != b'_' {
        return false;
    }
    let mut arm_start = start;
    while arm_start > 0 {
        let byte = buffer_byte(buffer, arm_start - 1);
        if matches!(byte, b'\n' | b';' | b'{' | b',') {
            break;
        }
        arm_start -= 1;
    }
    let mut before = arm_start;
    while before + 2 <= start {
        if code_range_equal(buffer, before, before + 2, b"if") {
            let mut after_if = before + 2;
            while after_if < start && buffer_byte(buffer, after_if).is_ascii_whitespace() {
                after_if += 1;
            }
            if !code_bytes_equal(buffer, after_if, b"let") {
                return false;
            }
        }
        before += 1;
    }
    let mut position = end;
    while position < length {
        let byte = buffer_byte(buffer, position);
        if byte == b'=' && position + 1 < length && buffer_byte(buffer, position + 1) == b'>' {
            return true;
        }
        if matches!(byte, b'\n' | b';' | b'{' | b',') {
            return false;
        }
        position += 1;
    }
    false
}

fn code_index_scan_block_comment(buffer: &GapBuffer, index: &mut CodeIndex, length: usize) {
    if code_bytes_equal(buffer, index.position, b"/*") {
        index.block_depth = index.block_depth.saturating_add(1);
        index.position += 2;
    } else if code_bytes_equal(buffer, index.position, b"*/") {
        index.block_depth = index.block_depth.saturating_sub(1);
        index.position += 2;
        if index.block_depth == 0 {
            index.mode = CodeIndexMode::Code;
        }
    } else {
        index.position = (index.position + 1).min(length);
    }
}

fn code_index_scan_string(buffer: &GapBuffer, index: &mut CodeIndex) {
    let byte = buffer_byte(buffer, index.position);
    if byte == b'\\' && !index.escaped {
        index.escaped = true;
        index.position += 1;
    } else if byte == index.delimiter && !index.escaped {
        index.position += 1;
        index.mode = CodeIndexMode::Code;
    } else {
        index.escaped = false;
        index.position += 1;
    }
}

pub fn code_index_identifier_at(index: &CodeIndex, position: usize) -> Option<usize> {
    let identifier = index
        .identifiers
        .partition_point(|identifier| identifier.end as usize <= position);
    index.identifiers.get(identifier).and_then(|candidate| {
        (candidate.start as usize <= position && position < candidate.end as usize)
            .then_some(identifier)
    })
}

pub fn code_index_definition_for(
    buffer: &GapBuffer,
    index: &CodeIndex,
    identifier: usize,
) -> Option<usize> {
    code_index_definition_for_kind(buffer, index, identifier, None)
}

pub fn code_index_definition_of_kind(
    buffer: &GapBuffer,
    index: &CodeIndex,
    identifier: usize,
    kind: CodeSymbolKind,
) -> Option<usize> {
    code_index_definition_for_kind(buffer, index, identifier, Some(kind))
}

fn code_index_definition_for_kind(
    buffer: &GapBuffer,
    index: &CodeIndex,
    identifier: usize,
    required_kind: Option<CodeSymbolKind>,
) -> Option<usize> {
    profiling::function_scope!();
    let Some(identifier) = index.identifiers.get(identifier).copied() else {
        return None;
    };
    let self_reference = code_range_equal(
        buffer,
        identifier.start as usize,
        identifier.end as usize,
        b"Self",
    );
    let mut best = None;
    for (symbol_index, symbol) in index.symbols.iter().enumerate() {
        if required_kind.is_some_and(|kind| symbol.kind != kind) {
            continue;
        }
        let Some(candidate) = index.identifiers.get(symbol.identifier as usize) else {
            continue;
        };
        let matching_definition = if self_reference {
            symbol.kind == CodeSymbolKind::Type
        } else {
            code_ranges_equal(buffer, identifier, *candidate)
        };
        if candidate.scope_depth > identifier.scope_depth || !matching_definition {
            continue;
        }
        let before = candidate.start <= identifier.start;
        let better = best.is_none_or(|best_symbol: usize| {
            let best_identifier = index.identifiers[index.symbols[best_symbol].identifier as usize];
            (before, candidate.scope_depth, candidate.start)
                > (
                    best_identifier.start <= identifier.start,
                    best_identifier.scope_depth,
                    best_identifier.start,
                )
        });
        if better {
            best = Some(symbol_index);
        }
    }
    best
}

pub fn code_ranges_equal(buffer: &GapBuffer, left: CodeIdentifier, right: CodeIdentifier) -> bool {
    let left_length = left.end - left.start;
    if left_length != right.end - right.start {
        return false;
    }
    (0..left_length as usize).all(|offset| {
        buffer_byte(buffer, left.start as usize + offset)
            == buffer_byte(buffer, right.start as usize + offset)
    })
}

fn code_definition_keyword(buffer: &GapBuffer, start: usize, end: usize) -> Option<CodeSymbolKind> {
    if code_range_equal(buffer, start, end, b"fn") {
        Some(CodeSymbolKind::Function)
    } else if code_range_equal(buffer, start, end, b"struct")
        || code_range_equal(buffer, start, end, b"enum")
        || code_range_equal(buffer, start, end, b"trait")
        || code_range_equal(buffer, start, end, b"type")
    {
        Some(CodeSymbolKind::Type)
    } else if code_range_equal(buffer, start, end, b"mod") {
        Some(CodeSymbolKind::Module)
    } else if code_range_equal(buffer, start, end, b"let") {
        Some(CodeSymbolKind::Value)
    } else if code_range_equal(buffer, start, end, b"const")
        || code_range_equal(buffer, start, end, b"static")
    {
        Some(CodeSymbolKind::Constant)
    } else {
        None
    }
}

fn code_rust_keyword(buffer: &GapBuffer, start: usize, end: usize) -> bool {
    [
        b"as".as_slice(),
        b"async",
        b"await",
        b"break",
        b"continue",
        b"crate",
        b"dyn",
        b"else",
        b"extern",
        b"false",
        b"for",
        b"if",
        b"impl",
        b"in",
        b"loop",
        b"match",
        b"move",
        b"mut",
        b"pub",
        b"ref",
        b"return",
        b"self",
        b"super",
        b"true",
        b"unsafe",
        b"use",
        b"where",
        b"while",
    ]
    .iter()
    .any(|keyword| code_range_equal(buffer, start, end, keyword))
}

#[inline]
fn code_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn code_next_nonwhitespace(buffer: &GapBuffer, mut position: usize, length: usize) -> Option<u8> {
    while position < length {
        let byte = buffer_byte(buffer, position);
        if !byte.is_ascii_whitespace() {
            return Some(byte);
        }
        position += 1;
    }
    None
}

fn code_bytes_equal(buffer: &GapBuffer, start: usize, expected: &[u8]) -> bool {
    start + expected.len() <= buffer_len(buffer)
        && expected
            .iter()
            .enumerate()
            .all(|(offset, &byte)| buffer_byte(buffer, start + offset) == byte)
}

fn code_range_equal(buffer: &GapBuffer, start: usize, end: usize, expected: &[u8]) -> bool {
    end - start == expected.len() && code_bytes_equal(buffer, start, expected)
}

fn code_rust_lifetime_end(buffer: &GapBuffer, start: usize, length: usize) -> usize {
    let mut end = start + 1;
    if end >= length || !code_identifier_byte(buffer_byte(buffer, end)) {
        return start;
    }
    end += 1;
    while end < length && code_identifier_byte(buffer_byte(buffer, end)) {
        end += 1;
    }
    if end < length && buffer_byte(buffer, end) == b'\'' {
        start
    } else {
        end
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::{buffer_delete, buffer_from_bytes, buffer_insert};

    #[test]
    fn local_definition_wins_over_outer_definition() {
        let source = b"let value = 1; { let value = 2; value } value";
        let buffer = buffer_from_bytes(source);
        let mut index = code_index_empty();
        code_index_set_path(&mut index, Some(std::path::Path::new("main.rs")));
        while !index.complete {
            code_index_step(&buffer, &mut index, 8, std::time::Duration::MAX);
        }
        let inner_reference = code_index_identifier_at(&index, 35).unwrap();
        let inner_definition = code_index_definition_for(&buffer, &index, inner_reference).unwrap();
        assert_eq!(
            index.identifiers[index.symbols[inner_definition].identifier as usize].start,
            21
        );
        let outer_reference = code_index_identifier_at(&index, 43).unwrap();
        let outer_definition = code_index_definition_for(&buffer, &index, outer_reference).unwrap();
        assert_eq!(
            index.identifiers[index.symbols[outer_definition].identifier as usize].start,
            4
        );
    }

    #[test]
    fn edit_invalidation_retains_the_prefix_and_resumes_at_a_checkpoint() {
        let original = b"fn first() {\n    let one = 1;\n}\nfn second() {\n    let two = one;\n}\n";
        let mut buffer = buffer_from_bytes(original);
        let mut index = code_index_empty();
        code_index_set_path(&mut index, Some(std::path::Path::new("main.rs")));
        code_index_step(&buffer, &mut index, usize::MAX, std::time::Duration::MAX);
        let edit = original.windows(3).position(|word| word == b"two").unwrap();
        let expected_restart = original[..edit]
            .iter()
            .rposition(|&byte| byte == b'\n')
            .unwrap()
            + 1;
        code_index_invalidate_edits(&mut index, &[(edit, edit + 3, 5)]);
        buffer_delete(&mut buffer, edit, edit + 3);
        buffer_insert(&mut buffer, edit, b"other");

        assert_eq!(index.position, expected_restart);
        assert!(index.position > 0);
        assert!(index.identifiers.iter().any(|identifier| {
            code_range_equal(
                &buffer,
                identifier.start as usize,
                identifier.end as usize,
                b"first",
            )
        }));
        while !index.complete {
            code_index_step(&buffer, &mut index, usize::MAX, std::time::Duration::MAX);
        }
        let mut rebuilt = code_index_empty();
        code_index_set_path(&mut rebuilt, Some(std::path::Path::new("main.rs")));
        code_index_step(&buffer, &mut rebuilt, usize::MAX, std::time::Duration::MAX);
        assert_eq!(index.identifiers, rebuilt.identifiers);
        assert_eq!(index.symbols, rebuilt.symbols);
    }

    #[test]
    fn mutable_local_resolves_inside_a_multiline_call() {
        let source = br#"fn open() {
    let mut document = document_empty();
    code_index_step(
        &document.buffer,
        &mut document.code_index,
        256 * 1024,
        std::time::Duration::from_millis(1),
    );
}"#;
        let buffer = buffer_from_bytes(source);
        let mut index = code_index_empty();
        code_index_set_path(&mut index, Some(std::path::Path::new("editor.rs")));
        while !index.complete {
            code_index_step(&buffer, &mut index, 32, std::time::Duration::MAX);
        }
        for use_position in source
            .windows(b"&document".len())
            .enumerate()
            .filter_map(|(position, word)| (word == b"&document").then_some(position + 1))
        {
            let identifier = code_index_identifier_at(&index, use_position).unwrap();
            let definition = code_index_definition_for(&buffer, &index, identifier).unwrap();
            let definition = index.identifiers[index.symbols[definition].identifier as usize];
            assert_eq!(
                &source[definition.start as usize..definition.end as usize],
                b"document"
            );
            assert_eq!(definition.start, 24);
        }
    }

    #[test]
    fn large_file_function_use_resolves_to_its_same_file_declaration() {
        let source = include_bytes!("editor.rs");
        let buffer = buffer_from_bytes(source);
        let mut index = code_index_empty();
        code_index_set_path(&mut index, Some(std::path::Path::new("editor.rs")));
        while !index.complete {
            code_index_step(&buffer, &mut index, 128 * 1024, std::time::Duration::MAX);
        }
        let needle = b"| editor_step_project_search(editor)";
        let use_position = source
            .windows(needle.len())
            .position(|window| window == needle)
            .unwrap()
            + 2;
        let identifier = code_index_identifier_at(&index, use_position).unwrap();
        let definition = code_index_definition_for(&buffer, &index, identifier).unwrap();
        let definition = index.identifiers[index.symbols[definition].identifier as usize];
        assert_eq!(
            &source[definition.start as usize..definition.end as usize],
            b"editor_step_project_search"
        );
        assert_ne!(definition.start, use_position as u32);
    }

    #[test]
    fn lifetimes_do_not_hide_following_constant_definitions() {
        let source = b"type Borrowed<'a> = &'a str; const LIMIT: usize = 4; LIMIT";
        let buffer = buffer_from_bytes(source);
        let mut index = code_index_empty();
        index.enabled = true;
        index.complete = false;
        code_index_step(&buffer, &mut index, usize::MAX, std::time::Duration::MAX);
        let use_position = source.len() - 1;
        let identifier = code_index_identifier_at(&index, use_position).unwrap();
        let definition = code_index_definition_for(&buffer, &index, identifier).unwrap();
        let symbol = index.symbols[definition];
        assert_eq!(symbol.kind, CodeSymbolKind::Constant);
        assert_eq!(index.identifiers[symbol.identifier as usize].start, 35);
    }

    #[test]
    fn constructor_patterns_bind_the_inner_value_in_let_and_match_arms() {
        for source in [
            b"if let Err(error) = call() { use_value(error); }".as_slice(),
            b"match call() { Err(error) => return Err(error), _ => {} }",
        ] {
            let buffer = buffer_from_bytes(source);
            let mut index = code_index_empty();
            index.enabled = true;
            index.complete = false;
            code_index_step(&buffer, &mut index, usize::MAX, std::time::Duration::MAX);
            let use_position = source
                .windows(5)
                .rposition(|word| word == b"error")
                .unwrap();
            let identifier = code_index_identifier_at(&index, use_position).unwrap();
            let definition = code_index_definition_for(&buffer, &index, identifier).unwrap();
            let definition = index.identifiers[index.symbols[definition].identifier as usize];
            assert!(definition.start < use_position as u32);
            assert_eq!(
                &source[definition.start as usize..definition.end as usize],
                b"error"
            );
        }
    }

    #[test]
    fn match_guard_uses_resolve_to_the_pattern_binding() {
        let source = b"match input { Some(path) if path.is_dir() => (path, None), _ => todo!() }";
        let buffer = buffer_from_bytes(source);
        let mut index = code_index_empty();
        index.enabled = true;
        index.complete = false;
        code_index_step(&buffer, &mut index, usize::MAX, std::time::Duration::MAX);
        let binding = source.windows(4).position(|word| word == b"path").unwrap();
        for use_position in source
            .windows(4)
            .enumerate()
            .filter_map(|(position, word)| {
                (word == b"path" && position != binding).then_some(position)
            })
        {
            let identifier = code_index_identifier_at(&index, use_position).unwrap();
            let definition = code_index_definition_for(&buffer, &index, identifier).unwrap();
            let definition = index.identifiers[index.symbols[definition].identifier as usize];
            assert_eq!(definition.start as usize, binding);
        }
    }

    #[test]
    fn argument_types_resolve_to_same_file_type_declarations() {
        let source = b"pub struct Editor {}\npub struct Terminal {}\npub fn editor_run(editor: &mut Editor, terminal: &mut Terminal) -> std::io::Result<()> {}";
        let buffer = buffer_from_bytes(source);
        let mut index = code_index_empty();
        index.enabled = true;
        index.complete = false;
        code_index_step(&buffer, &mut index, usize::MAX, std::time::Duration::MAX);
        for name in [b"Editor".as_slice(), b"Terminal"] {
            let use_position = source
                .windows(name.len())
                .rposition(|word| word == name)
                .unwrap();
            let declaration = source
                .windows(name.len())
                .position(|word| word == name)
                .unwrap();
            let identifier = code_index_identifier_at(&index, use_position).unwrap();
            let definition = code_index_definition_for(&buffer, &index, identifier).unwrap();
            let definition = index.identifiers[index.symbols[definition].identifier as usize];
            assert_eq!(definition.start as usize, declaration);
        }
    }

    #[test]
    fn implicit_self_type_resolves_to_the_enclosing_trait() {
        let source = b"pub trait DynamicsValue: Copy + Add<Output = Self> {}";
        let buffer = buffer_from_bytes(source);
        let mut index = code_index_empty();
        index.enabled = true;
        index.complete = false;
        code_index_step(&buffer, &mut index, usize::MAX, std::time::Duration::MAX);
        let self_position = source.windows(4).position(|word| word == b"Self").unwrap();
        let identifier = code_index_identifier_at(&index, self_position).unwrap();
        let definition = code_index_definition_for(&buffer, &index, identifier).unwrap();
        let definition = index.identifiers[index.symbols[definition].identifier as usize];
        assert_eq!(
            &source[definition.start as usize..definition.end as usize],
            b"DynamicsValue"
        );
    }
}
