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

pub fn code_index_empty() -> CodeIndex {
    CodeIndex {
        identifiers: Vec::new(),
        symbols: Vec::new(),
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
    *position = 0;
    *scope_depth = 0;
    *block_depth = 0;
    *mode = CodeIndexMode::Code;
    *delimiter = 0;
    *escaped = false;
    *expected_symbol = None;
    *complete = !*enabled;
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
    }
    if index.position >= length {
        index.complete = true;
    }
    index.identifiers.len() != original_identifiers
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
    } else if byte == b'\''
        && code_rust_lifetime_end(buffer, index.position, length) > index.position
    {
        index.position = code_rust_lifetime_end(buffer, index.position, length);
    } else if matches!(byte, b'\'' | b'"') {
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
    if let Some(kind) = index.expected_symbol.take() {
        index.symbols.push(CodeSymbol {
            identifier: identifier as u32,
            kind,
        });
    } else if followed_by_field_colon {
        index.symbols.push(CodeSymbol {
            identifier: identifier as u32,
            kind: CodeSymbolKind::Value,
        });
    }
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
    profiling::function_scope!();
    let Some(identifier) = index.identifiers.get(identifier).copied() else {
        return None;
    };
    let mut best = None;
    for (symbol_index, symbol) in index.symbols.iter().enumerate() {
        let Some(candidate) = index.identifiers.get(symbol.identifier as usize) else {
            continue;
        };
        if candidate.scope_depth > identifier.scope_depth
            || !code_ranges_equal(buffer, identifier, *candidate)
        {
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
        b"Self",
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
    use crate::buffer::buffer_from_bytes;

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
}
