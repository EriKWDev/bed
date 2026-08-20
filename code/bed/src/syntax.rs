use crate::buffer::{GapBuffer, buffer_byte, buffer_len};

pub const SYNTAX_KIND_COUNT: usize = 18;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SyntaxKind {
    Comment,
    Keyword,
    String,
    Number,
    Type,
    Function,
    Attribute,
    Markup,
    CommentAnnotation,
    Control,
    Declaration,
    Constant,
    Operator,
    Punctuation,
    Lifetime,
    CommentNote,
    CommentWarning,
    CommentError,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyntaxLanguage {
    None,
    Toml,
    Markdown,
    Rust,
    C,
    Cpp,
    Go,
    Jai,
    Nim,
    Odin,
    Shell,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyntaxSpan {
    pub start: u32,
    pub end: u32,
    pub kind: SyntaxKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyntaxMode {
    Code,
    Identifier,
    Number,
    LineComment,
    BlockComment,
    String,
    MarkupLine,
    MarkupDelimited,
    MarkdownFence,
}

pub struct SyntaxHighlighting {
    pub language: SyntaxLanguage,
    pub spans: Vec<SyntaxSpan>,
    pub previous_spans: Vec<SyntaxSpan>,
    pub checkpoints: Vec<SyntaxCheckpoint>,
    pub position: usize,
    pub token_start: usize,
    pub mode: SyntaxMode,
    pub delimiter: u8,
    pub delimiter_length: u8,
    pub block_depth: u16,
    pub escaped: bool,
    pub line_start: bool,
    pub complete: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyntaxCheckpoint {
    pub position: u32,
    pub token_start: u32,
    pub block_depth: u16,
    pub mode: SyntaxMode,
    pub delimiter: u8,
    pub delimiter_length: u8,
    pub escaped: bool,
    pub line_start: bool,
}

pub fn syntax_highlighting_empty() -> SyntaxHighlighting {
    SyntaxHighlighting {
        language: SyntaxLanguage::None,
        spans: Vec::new(),
        previous_spans: Vec::new(),
        checkpoints: Vec::new(),
        position: 0,
        token_start: 0,
        mode: SyntaxMode::Code,
        delimiter: 0,
        delimiter_length: 1,
        block_depth: 0,
        escaped: false,
        line_start: true,
        complete: true,
    }
}

pub fn syntax_language_from_path(path: Option<&std::path::Path>) -> SyntaxLanguage {
    if path
        .and_then(std::path::Path::file_name)
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|name| matches!(name, ".bashrc" | ".bash_profile" | ".zshrc" | ".zprofile"))
    {
        return SyntaxLanguage::Shell;
    }
    let Some(extension) = path.and_then(std::path::Path::extension) else {
        return SyntaxLanguage::None;
    };
    let Some(extension) = extension.to_str() else {
        return SyntaxLanguage::None;
    };
    match extension {
        "toml" => SyntaxLanguage::Toml,
        "md" | "markdown" => SyntaxLanguage::Markdown,
        "rs" => SyntaxLanguage::Rust,
        "c" | "h" => SyntaxLanguage::C,
        "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" => SyntaxLanguage::Cpp,
        "go" => SyntaxLanguage::Go,
        "jai" => SyntaxLanguage::Jai,
        "nim" | "nims" | "nimble" => SyntaxLanguage::Nim,
        "odin" => SyntaxLanguage::Odin,
        "sh" | "bash" | "zsh" => SyntaxLanguage::Shell,
        _ => SyntaxLanguage::None,
    }
}

pub fn syntax_highlighting_set_path(
    highlighting: &mut SyntaxHighlighting,
    path: Option<&std::path::Path>,
) {
    profiling::function_scope!();
    highlighting.spans.clear();
    highlighting.previous_spans.clear();
    highlighting.language = syntax_language_from_path(path);
    syntax_highlighting_invalidate(highlighting);
}

pub fn syntax_highlighting_invalidate(highlighting: &mut SyntaxHighlighting) {
    profiling::function_scope!();
    let SyntaxHighlighting {
        language,
        spans,
        previous_spans,
        checkpoints,
        position,
        token_start,
        mode,
        delimiter,
        delimiter_length,
        block_depth,
        escaped,
        line_start,
        complete,
    } = highlighting;
    if previous_spans.is_empty() && !spans.is_empty() {
        std::mem::swap(spans, previous_spans);
    }
    spans.clear();
    *position = 0;
    *token_start = 0;
    *mode = SyntaxMode::Code;
    *delimiter = 0;
    *delimiter_length = 1;
    *block_depth = 0;
    *escaped = false;
    *line_start = true;
    *complete = *language == SyntaxLanguage::None;
    checkpoints.clear();
    checkpoints.push(SyntaxCheckpoint {
        position: 0,
        token_start: 0,
        block_depth: 0,
        mode: SyntaxMode::Code,
        delimiter: 0,
        delimiter_length: 1,
        escaped: false,
        line_start: true,
    });
}

pub fn syntax_highlighting_step(
    buffer: &GapBuffer,
    highlighting: &mut SyntaxHighlighting,
    maximum_bytes: usize,
    maximum_time: std::time::Duration,
) -> bool {
    profiling::function_scope!();
    if highlighting.complete || highlighting.language == SyntaxLanguage::None {
        return false;
    }
    let original_span_count = highlighting.spans.len();
    let original_position = highlighting.position;
    let retained_previous = !highlighting.previous_spans.is_empty();
    let length = buffer_len(buffer).min(u32::MAX as usize);
    let end = highlighting
        .position
        .saturating_add(maximum_bytes)
        .min(length);
    let started = std::time::Instant::now();

    profiling::scope!("scan syntax bytes");
    while highlighting.position < end {
        if highlighting.position & 255 == 0 && started.elapsed() >= maximum_time {
            break;
        }
        let previous_position = highlighting.position;
        match highlighting.mode {
            SyntaxMode::Code => syntax_scan_code_byte(buffer, highlighting, length),
            SyntaxMode::Identifier => syntax_scan_identifier_byte(buffer, highlighting, length),
            SyntaxMode::Number => syntax_scan_number_byte(buffer, highlighting, length),
            SyntaxMode::LineComment => {
                syntax_scan_line_span_byte(buffer, highlighting, SyntaxKind::Comment)
            }
            SyntaxMode::BlockComment => syntax_scan_block_comment_byte(buffer, highlighting),
            SyntaxMode::String => syntax_scan_string_byte(buffer, highlighting),
            SyntaxMode::MarkupLine => {
                syntax_scan_line_span_byte(buffer, highlighting, SyntaxKind::Markup)
            }
            SyntaxMode::MarkupDelimited => syntax_scan_markup_delimited_byte(buffer, highlighting),
            SyntaxMode::MarkdownFence => syntax_scan_markdown_fence_byte(buffer, highlighting),
        }
        if highlighting.position > previous_position
            && buffer_byte(buffer, highlighting.position - 1) == b'\n'
        {
            syntax_checkpoint_push(highlighting);
        }
    }

    if highlighting.position >= length {
        syntax_highlighting_finish(buffer, highlighting, length);
        highlighting.complete = true;
        highlighting.previous_spans.clear();
        return true;
    }
    !retained_previous
        && (highlighting.position != original_position
            || highlighting.spans.len() != original_span_count)
}

pub fn syntax_highlighting_invalidate_edits(
    highlighting: &mut SyntaxHighlighting,
    edits: &[(usize, usize, usize)],
) {
    profiling::function_scope!();
    if edits.is_empty() {
        return;
    }
    if highlighting.previous_spans.is_empty() && !highlighting.spans.is_empty() {
        std::mem::swap(&mut highlighting.spans, &mut highlighting.previous_spans);
    }
    syntax_highlighting_adjust_edits(highlighting, edits);

    let first_edit = edits.iter().map(|&(start, _, _)| start).min().unwrap_or(0);
    let checkpoint_index = highlighting
        .checkpoints
        .partition_point(|checkpoint| checkpoint.position as usize <= first_edit)
        .saturating_sub(1);
    let checkpoint = highlighting
        .checkpoints
        .get(checkpoint_index)
        .copied()
        .unwrap_or(SyntaxCheckpoint {
            position: 0,
            token_start: 0,
            block_depth: 0,
            mode: SyntaxMode::Code,
            delimiter: 0,
            delimiter_length: 1,
            escaped: false,
            line_start: true,
        });
    let restart = checkpoint.position as usize;
    highlighting
        .spans
        .retain(|span| span.end as usize <= restart);
    if highlighting.spans.is_empty() && restart > 0 {
        highlighting.spans.extend(
            highlighting
                .previous_spans
                .iter()
                .copied()
                .take_while(|span| span.end as usize <= restart),
        );
    }
    highlighting.checkpoints.truncate(checkpoint_index + 1);
    highlighting.position = restart;
    highlighting.token_start = checkpoint.token_start as usize;
    highlighting.block_depth = checkpoint.block_depth;
    highlighting.mode = checkpoint.mode;
    highlighting.delimiter = checkpoint.delimiter;
    highlighting.delimiter_length = checkpoint.delimiter_length;
    highlighting.escaped = checkpoint.escaped;
    highlighting.line_start = checkpoint.line_start;
    highlighting.complete = highlighting.language == SyntaxLanguage::None;
}

fn syntax_checkpoint_push(highlighting: &mut SyntaxHighlighting) {
    let checkpoint = SyntaxCheckpoint {
        position: highlighting.position as u32,
        token_start: highlighting.token_start as u32,
        block_depth: highlighting.block_depth,
        mode: highlighting.mode,
        delimiter: highlighting.delimiter,
        delimiter_length: highlighting.delimiter_length,
        escaped: highlighting.escaped,
        line_start: highlighting.line_start,
    };
    if highlighting
        .checkpoints
        .last()
        .is_none_or(|previous| previous.position < checkpoint.position)
    {
        highlighting.checkpoints.push(checkpoint);
    }
}

pub fn syntax_highlighting_spans(highlighting: &SyntaxHighlighting) -> &[SyntaxSpan] {
    if !highlighting.complete && !highlighting.previous_spans.is_empty() {
        &highlighting.previous_spans
    } else {
        &highlighting.spans
    }
}

pub fn syntax_highlighting_adjust_edits(
    highlighting: &mut SyntaxHighlighting,
    edits: &[(usize, usize, usize)],
) {
    profiling::function_scope!();
    let spans = if !highlighting.previous_spans.is_empty() {
        &mut highlighting.previous_spans
    } else {
        &mut highlighting.spans
    };
    let mut shift = 0isize;
    for &(original_start, original_end, inserted_length) in edits {
        let start = (original_start as isize + shift) as usize;
        let end = (original_end as isize + shift) as usize;
        let delta = inserted_length as isize - (original_end - original_start) as isize;
        spans.retain_mut(|span| {
            let span_start = span.start as usize;
            let span_end = span.end as usize;
            if span_end <= start {
                true
            } else if span_start >= end {
                span.start = (span_start as isize + delta) as u32;
                span.end = (span_end as isize + delta) as u32;
                true
            } else {
                false
            }
        });
        shift += delta;
    }
}

fn syntax_scan_code_byte(buffer: &GapBuffer, highlighting: &mut SyntaxHighlighting, length: usize) {
    let position = highlighting.position;
    let byte = buffer_byte(buffer, position);
    let next = (position + 1 < length).then(|| buffer_byte(buffer, position + 1));

    if byte == b'\n' {
        highlighting.position += 1;
        highlighting.line_start = true;
        return;
    }
    if byte.is_ascii_whitespace() {
        highlighting.position += 1;
        return;
    }

    if highlighting.language == SyntaxLanguage::Markdown {
        if highlighting.line_start && byte == b'#' {
            syntax_mode_begin(highlighting, SyntaxMode::MarkupLine, position, 0, 1);
            return;
        }
        if highlighting.line_start && matches!(byte, b'>' | b'-' | b'*' | b'+') {
            syntax_mode_begin(highlighting, SyntaxMode::MarkupLine, position, 0, 1);
            return;
        }
        if byte == b'`' {
            let delimiter_length = if syntax_bytes_equal(buffer, position, b"```") {
                3
            } else {
                1
            };
            let mode = if delimiter_length == 3 {
                SyntaxMode::MarkdownFence
            } else {
                SyntaxMode::MarkupDelimited
            };
            syntax_mode_begin(highlighting, mode, position, b'`', delimiter_length);
            highlighting.position += delimiter_length as usize;
            if delimiter_length == 3 {
                syntax_span_push(
                    highlighting,
                    position,
                    highlighting.position,
                    SyntaxKind::Markup,
                );
            }
            highlighting.line_start = false;
            return;
        }
        if syntax_bytes_equal(buffer, position, b"<!--") {
            syntax_mode_begin(highlighting, SyntaxMode::BlockComment, position, 0, 1);
            highlighting.position += 4;
            return;
        }
        if byte == b'[' {
            syntax_mode_begin(highlighting, SyntaxMode::MarkupDelimited, position, b')', 1);
            highlighting.position += 1;
            highlighting.line_start = false;
            return;
        }
        if matches!(byte, b'*' | b'_') {
            syntax_mode_begin(highlighting, SyntaxMode::MarkupDelimited, position, byte, 1);
            highlighting.position += 1;
            highlighting.line_start = false;
            return;
        }
        highlighting.position += 1;
        highlighting.line_start = false;
        return;
    }

    if syntax_starts_line_comment(highlighting.language, byte, next) {
        syntax_mode_begin(highlighting, SyntaxMode::LineComment, position, 0, 1);
        highlighting.position += usize::from(next == Some(byte));
        highlighting.line_start = false;
        return;
    }
    if syntax_starts_block_comment(highlighting.language, byte, next) {
        syntax_mode_begin(highlighting, SyntaxMode::BlockComment, position, 0, 1);
        highlighting.block_depth = 1;
        highlighting.position += 2;
        highlighting.line_start = false;
        return;
    }
    if highlighting.language == SyntaxLanguage::Rust && byte == b'\'' {
        let lifetime_end = syntax_rust_lifetime_end(buffer, position, length);
        if lifetime_end > position {
            syntax_span_push(highlighting, position, lifetime_end, SyntaxKind::Lifetime);
            highlighting.position = lifetime_end;
            highlighting.line_start = false;
            return;
        }
    }
    if syntax_starts_string(highlighting.language, byte) {
        let delimiter_length = if matches!(
            highlighting.language,
            SyntaxLanguage::Toml | SyntaxLanguage::Nim
        ) && position + 2 < length
            && buffer_byte(buffer, position + 1) == byte
            && buffer_byte(buffer, position + 2) == byte
        {
            3
        } else {
            1
        };
        syntax_mode_begin(
            highlighting,
            SyntaxMode::String,
            position,
            byte,
            delimiter_length,
        );
        highlighting.position += delimiter_length as usize;
        highlighting.line_start = false;
        return;
    }
    if syntax_starts_attribute(highlighting.language, highlighting.line_start, byte, next) {
        syntax_mode_begin(highlighting, SyntaxMode::MarkupLine, position, 0, 1);
        highlighting.line_start = false;
        return;
    }
    if byte.is_ascii_digit() {
        syntax_mode_begin(highlighting, SyntaxMode::Number, position, 0, 1);
        highlighting.position += 1;
        highlighting.line_start = false;
        return;
    }
    if syntax_identifier_byte(byte) {
        syntax_mode_begin(highlighting, SyntaxMode::Identifier, position, 0, 1);
        highlighting.position += 1;
        highlighting.line_start = false;
        return;
    }
    if matches!(
        byte,
        b'+' | b'-'
            | b'*'
            | b'/'
            | b'%'
            | b'='
            | b'!'
            | b'&'
            | b'|'
            | b'^'
            | b'<'
            | b'>'
            | b'$'
            | b'~'
    ) {
        syntax_span_push(highlighting, position, position + 1, SyntaxKind::Operator);
    } else if matches!(
        byte,
        b'(' | b')' | b'[' | b']' | b'{' | b'}' | b',' | b';' | b':' | b'.'
    ) {
        syntax_span_push(
            highlighting,
            position,
            position + 1,
            SyntaxKind::Punctuation,
        );
    }
    highlighting.position += 1;
    highlighting.line_start = false;
}

fn syntax_scan_identifier_byte(
    buffer: &GapBuffer,
    highlighting: &mut SyntaxHighlighting,
    length: usize,
) {
    if highlighting.position < length
        && syntax_identifier_byte(buffer_byte(buffer, highlighting.position))
    {
        highlighting.position += 1;
        return;
    }
    let end = highlighting.position;
    let kind = syntax_identifier_kind(
        buffer,
        highlighting.language,
        highlighting.token_start,
        end,
        length,
    );
    if let Some(kind) = kind {
        syntax_span_push(highlighting, highlighting.token_start, end, kind);
    }
    highlighting.mode = SyntaxMode::Code;
}

fn syntax_scan_number_byte(
    buffer: &GapBuffer,
    highlighting: &mut SyntaxHighlighting,
    length: usize,
) {
    if highlighting.position < length {
        let byte = buffer_byte(buffer, highlighting.position);
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b'_' | b'.' | b'+' | b'-')
            || (highlighting.language == SyntaxLanguage::Toml && byte == b':')
        {
            highlighting.position += 1;
            return;
        }
    }
    let kind = syntax_number_kind(
        buffer,
        highlighting.language,
        highlighting.token_start,
        highlighting.position,
    );
    syntax_span_push(
        highlighting,
        highlighting.token_start,
        highlighting.position,
        kind,
    );
    highlighting.mode = SyntaxMode::Code;
}

fn syntax_scan_line_span_byte(
    buffer: &GapBuffer,
    highlighting: &mut SyntaxHighlighting,
    kind: SyntaxKind,
) {
    let byte = buffer_byte(buffer, highlighting.position);
    if byte == b'\n' {
        if kind == SyntaxKind::Comment {
            syntax_comment_spans_push(
                buffer,
                highlighting,
                highlighting.token_start,
                highlighting.position,
            );
        } else {
            syntax_span_push(
                highlighting,
                highlighting.token_start,
                highlighting.position,
                kind,
            );
        }
        highlighting.mode = SyntaxMode::Code;
        return;
    }
    highlighting.position += 1;
}

fn syntax_scan_block_comment_byte(buffer: &GapBuffer, highlighting: &mut SyntaxHighlighting) {
    let position = highlighting.position;
    let length = buffer_len(buffer).min(u32::MAX as usize);
    if highlighting.language == SyntaxLanguage::Markdown {
        if syntax_bytes_equal(buffer, position, b"-->") {
            highlighting.position += 3;
            syntax_span_push(
                highlighting,
                highlighting.token_start,
                highlighting.position,
                SyntaxKind::Comment,
            );
            highlighting.mode = SyntaxMode::Code;
            return;
        }
    } else if highlighting.language == SyntaxLanguage::Nim {
        if syntax_bytes_equal(buffer, position, b"#[") {
            highlighting.block_depth = highlighting.block_depth.saturating_add(1);
            highlighting.position += 2;
            return;
        }
        if syntax_bytes_equal(buffer, position, b"]#") {
            highlighting.block_depth = highlighting.block_depth.saturating_sub(1);
            highlighting.position += 2;
            if highlighting.block_depth == 0 {
                syntax_comment_spans_push(
                    buffer,
                    highlighting,
                    highlighting.token_start,
                    highlighting.position,
                );
                highlighting.mode = SyntaxMode::Code;
            }
            return;
        }
    } else {
        if syntax_bytes_equal(buffer, position, b"/*")
            && matches!(
                highlighting.language,
                SyntaxLanguage::Rust | SyntaxLanguage::Odin
            )
        {
            highlighting.block_depth = highlighting.block_depth.saturating_add(1);
            highlighting.position += 2;
            return;
        }
        if syntax_bytes_equal(buffer, position, b"*/") {
            highlighting.block_depth = highlighting.block_depth.saturating_sub(1);
            highlighting.position += 2;
            if highlighting.block_depth == 0 {
                syntax_comment_spans_push(
                    buffer,
                    highlighting,
                    highlighting.token_start,
                    highlighting.position,
                );
                highlighting.mode = SyntaxMode::Code;
            }
            return;
        }
    }
    if position < length {
        highlighting.line_start = buffer_byte(buffer, position) == b'\n';
        highlighting.position += 1;
    }
}

fn syntax_scan_string_byte(buffer: &GapBuffer, highlighting: &mut SyntaxHighlighting) {
    let byte = buffer_byte(buffer, highlighting.position);
    let raw = highlighting.delimiter == b'`'
        || (highlighting.language == SyntaxLanguage::Toml && highlighting.delimiter == b'\'');
    if !raw && byte == b'\\' && !highlighting.escaped {
        highlighting.escaped = true;
        highlighting.position += 1;
        return;
    }
    if byte == highlighting.delimiter && !highlighting.escaped {
        let closes = highlighting.delimiter_length == 1
            || syntax_repeated_byte(
                buffer,
                highlighting.position,
                highlighting.delimiter,
                highlighting.delimiter_length as usize,
            );
        if closes {
            highlighting.position += highlighting.delimiter_length as usize;
            let kind = syntax_toml_string_key_kind(
                buffer,
                highlighting.language,
                highlighting.token_start,
                highlighting.position,
            )
            .unwrap_or(SyntaxKind::String);
            syntax_span_push(
                highlighting,
                highlighting.token_start,
                highlighting.position,
                kind,
            );
            highlighting.mode = SyntaxMode::Code;
            highlighting.escaped = false;
            return;
        }
    }
    if byte == b'\n' && highlighting.delimiter_length == 1 && highlighting.delimiter != b'`' {
        syntax_span_push(
            highlighting,
            highlighting.token_start,
            highlighting.position,
            SyntaxKind::String,
        );
        highlighting.mode = SyntaxMode::Code;
        highlighting.escaped = false;
        return;
    }
    highlighting.escaped = false;
    highlighting.line_start = byte == b'\n';
    highlighting.position += 1;
}

fn syntax_scan_markup_delimited_byte(buffer: &GapBuffer, highlighting: &mut SyntaxHighlighting) {
    let byte = buffer_byte(buffer, highlighting.position);
    if byte == highlighting.delimiter {
        highlighting.position += 1;
        syntax_span_push(
            highlighting,
            highlighting.token_start,
            highlighting.position,
            SyntaxKind::Markup,
        );
        highlighting.mode = SyntaxMode::Code;
        return;
    }
    if byte == b'\n' {
        syntax_span_push(
            highlighting,
            highlighting.token_start,
            highlighting.position,
            SyntaxKind::Markup,
        );
        highlighting.mode = SyntaxMode::Code;
        return;
    }
    highlighting.position += 1;
}

fn syntax_scan_markdown_fence_byte(buffer: &GapBuffer, highlighting: &mut SyntaxHighlighting) {
    let byte = buffer_byte(buffer, highlighting.position);
    if highlighting.line_start && syntax_bytes_equal(buffer, highlighting.position, b"```") {
        let start = highlighting.position;
        highlighting.position += 3;
        syntax_span_push(
            highlighting,
            start,
            highlighting.position,
            SyntaxKind::Markup,
        );
        highlighting.mode = SyntaxMode::Code;
        return;
    }
    highlighting.line_start = byte == b'\n';
    highlighting.position += 1;
}

fn syntax_highlighting_finish(
    buffer: &GapBuffer,
    highlighting: &mut SyntaxHighlighting,
    length: usize,
) {
    match highlighting.mode {
        SyntaxMode::Identifier => {
            let kind = syntax_identifier_kind(
                buffer,
                highlighting.language,
                highlighting.token_start,
                length,
                length,
            );
            if let Some(kind) = kind {
                syntax_span_push(highlighting, highlighting.token_start, length, kind);
            }
        }
        SyntaxMode::Number => {
            let kind = syntax_number_kind(
                buffer,
                highlighting.language,
                highlighting.token_start,
                length,
            );
            syntax_span_push(highlighting, highlighting.token_start, length, kind);
        }
        SyntaxMode::LineComment | SyntaxMode::BlockComment => {
            syntax_comment_spans_push(buffer, highlighting, highlighting.token_start, length)
        }
        SyntaxMode::String => syntax_span_push(
            highlighting,
            highlighting.token_start,
            length,
            SyntaxKind::String,
        ),
        SyntaxMode::MarkupLine | SyntaxMode::MarkupDelimited => syntax_span_push(
            highlighting,
            highlighting.token_start,
            length,
            SyntaxKind::Markup,
        ),
        SyntaxMode::Code | SyntaxMode::MarkdownFence => {}
    }
    highlighting.mode = SyntaxMode::Code;
    highlighting.position = length;
}

fn syntax_mode_begin(
    highlighting: &mut SyntaxHighlighting,
    mode: SyntaxMode,
    token_start: usize,
    delimiter: u8,
    delimiter_length: u8,
) {
    highlighting.mode = mode;
    highlighting.token_start = token_start;
    highlighting.delimiter = delimiter;
    highlighting.delimiter_length = delimiter_length;
    highlighting.escaped = false;
}

fn syntax_span_push(
    highlighting: &mut SyntaxHighlighting,
    start: usize,
    end: usize,
    kind: SyntaxKind,
) {
    if start >= end || end > u32::MAX as usize {
        return;
    }
    highlighting.spans.push(SyntaxSpan {
        start: start as u32,
        end: end as u32,
        kind,
    });
}

fn syntax_comment_spans_push(
    buffer: &GapBuffer,
    highlighting: &mut SyntaxHighlighting,
    start: usize,
    end: usize,
) {
    profiling::function_scope!();
    let mut plain_start = start;
    let mut position = start;
    while position < end {
        let byte = buffer_byte(buffer, position);
        let boundary =
            position == start || !syntax_identifier_byte(buffer_byte(buffer, position - 1));
        if !boundary || !byte.is_ascii_uppercase() {
            position += 1;
            continue;
        }
        let marker_start = position;
        while position < end {
            let byte = buffer_byte(buffer, position);
            if !byte.is_ascii_uppercase() && !byte.is_ascii_digit() && byte != b'_' {
                break;
            }
            position += 1;
        }
        if position == marker_start {
            position += 1;
            continue;
        }
        if position < end && matches!(buffer_byte(buffer, position), b'(' | b'[') {
            let open = buffer_byte(buffer, position);
            let close = if open == b'(' { b')' } else { b']' };
            position += 1;
            while position < end && buffer_byte(buffer, position) != close {
                position += 1;
            }
            position += usize::from(position < end);
        }
        if position >= end || buffer_byte(buffer, position) != b':' {
            position = marker_start + 1;
            continue;
        }
        position += 1;
        syntax_span_push(highlighting, plain_start, marker_start, SyntaxKind::Comment);
        let kind = syntax_comment_annotation_kind(buffer, marker_start, position);
        syntax_span_push(highlighting, marker_start, position, kind);
        plain_start = position;
    }
    syntax_span_push(highlighting, plain_start, end, SyntaxKind::Comment);
}

fn syntax_comment_annotation_kind(buffer: &GapBuffer, start: usize, end: usize) -> SyntaxKind {
    let mut name_end = start;
    while name_end < end {
        let byte = buffer_byte(buffer, name_end);
        if !byte.is_ascii_uppercase() && !byte.is_ascii_digit() && byte != b'_' {
            break;
        }
        name_end += 1;
    }
    for name in [b"FIXME".as_slice(), b"ERROR", b"BUG", b"FATAL"] {
        if syntax_range_equal(buffer, start, name_end, name) {
            return SyntaxKind::CommentError;
        }
    }
    for name in [b"WARNING".as_slice(), b"WARN", b"HACK", b"SAFETY"] {
        if syntax_range_equal(buffer, start, name_end, name) {
            return SyntaxKind::CommentWarning;
        }
    }
    for name in [b"NOTE".as_slice(), b"HELP", b"INFO", b"TODO"] {
        if syntax_range_equal(buffer, start, name_end, name) {
            return SyntaxKind::CommentNote;
        }
    }
    SyntaxKind::CommentAnnotation
}

fn syntax_rust_lifetime_end(buffer: &GapBuffer, start: usize, length: usize) -> usize {
    let mut end = start + 1;
    if end >= length || !syntax_identifier_byte(buffer_byte(buffer, end)) {
        return start;
    }
    end += 1;
    while end < length && syntax_identifier_byte(buffer_byte(buffer, end)) {
        end += 1;
    }
    if end < length && buffer_byte(buffer, end) == b'\'' {
        start
    } else {
        end
    }
}

fn syntax_control_keyword(
    buffer: &GapBuffer,
    _language: SyntaxLanguage,
    start: usize,
    end: usize,
) -> bool {
    [
        b"break".as_slice(),
        b"case",
        b"catch",
        b"continue",
        b"defer",
        b"do",
        b"else",
        b"except",
        b"finally",
        b"for",
        b"if",
        b"loop",
        b"match",
        b"raise",
        b"return",
        b"select",
        b"switch",
        b"throw",
        b"try",
        b"when",
        b"while",
        b"yield",
    ]
    .iter()
    .any(|word| syntax_range_equal(buffer, start, end, word))
}

fn syntax_declaration_keyword(
    buffer: &GapBuffer,
    _language: SyntaxLanguage,
    start: usize,
    end: usize,
) -> bool {
    [
        b"class".as_slice(),
        b"const",
        b"enum",
        b"fn",
        b"function",
        b"func",
        b"interface",
        b"let",
        b"mod",
        b"namespace",
        b"proc",
        b"static",
        b"struct",
        b"trait",
        b"type",
        b"typedef",
        b"union",
        b"var",
    ]
    .iter()
    .any(|word| syntax_range_equal(buffer, start, end, word))
}

fn syntax_previous_word_is(buffer: &GapBuffer, start: usize, expected: &[u8]) -> bool {
    let mut end = start;
    while end > 0 && buffer_byte(buffer, end - 1).is_ascii_whitespace() {
        end -= 1;
    }
    let mut previous_start = end;
    while previous_start > 0 && syntax_identifier_byte(buffer_byte(buffer, previous_start - 1)) {
        previous_start -= 1;
    }
    syntax_range_equal(buffer, previous_start, end, expected)
}

fn syntax_number_kind(
    buffer: &GapBuffer,
    language: SyntaxLanguage,
    start: usize,
    end: usize,
) -> SyntaxKind {
    if language == SyntaxLanguage::Toml
        && (start..end)
            .any(|position| matches!(buffer_byte(buffer, position), b'-' | b':' | b'T' | b'Z'))
    {
        SyntaxKind::String
    } else {
        SyntaxKind::Number
    }
}

fn syntax_toml_bare_key_kind(
    buffer: &GapBuffer,
    start: usize,
    end: usize,
    length: usize,
) -> Option<SyntaxKind> {
    syntax_toml_key_kind(buffer, start, end, length)
}

fn syntax_toml_string_key_kind(
    buffer: &GapBuffer,
    language: SyntaxLanguage,
    start: usize,
    end: usize,
) -> Option<SyntaxKind> {
    if language != SyntaxLanguage::Toml {
        return None;
    }
    syntax_toml_key_kind(buffer, start, end, buffer_len(buffer))
}

fn syntax_toml_key_kind(
    buffer: &GapBuffer,
    start: usize,
    end: usize,
    length: usize,
) -> Option<SyntaxKind> {
    let mut line_start = start;
    while line_start > 0 && buffer_byte(buffer, line_start - 1) != b'\n' {
        line_start -= 1;
    }
    let mut first = line_start;
    while first < length && matches!(buffer_byte(buffer, first), b' ' | b'\t') {
        first += 1;
    }
    if first < start && buffer_byte(buffer, first) == b'[' {
        return Some(SyntaxKind::Type);
    }
    if (line_start..start).any(|position| buffer_byte(buffer, position) == b'=') {
        return None;
    }
    let mut position = end;
    while position < length {
        match buffer_byte(buffer, position) {
            b'=' => return Some(SyntaxKind::Attribute),
            b'\n' | b'#' => return None,
            _ => position += 1,
        }
    }
    None
}

fn syntax_identifier_kind(
    buffer: &GapBuffer,
    language: SyntaxLanguage,
    start: usize,
    end: usize,
    length: usize,
) -> Option<SyntaxKind> {
    if language == SyntaxLanguage::Toml {
        if syntax_range_equal(buffer, start, end, b"true")
            || syntax_range_equal(buffer, start, end, b"false")
        {
            return Some(SyntaxKind::Constant);
        }
        if let Some(kind) = syntax_toml_bare_key_kind(buffer, start, end, length) {
            return Some(kind);
        }
    }
    if syntax_control_keyword(buffer, language, start, end) {
        return Some(SyntaxKind::Control);
    }
    if syntax_declaration_keyword(buffer, language, start, end) {
        return Some(SyntaxKind::Declaration);
    }
    if syntax_keyword(buffer, language, start, end) {
        return Some(SyntaxKind::Keyword);
    }
    if syntax_previous_word_is(buffer, start, b"fn")
        || syntax_previous_word_is(buffer, start, b"func")
        || syntax_previous_word_is(buffer, start, b"proc")
    {
        return Some(SyntaxKind::Function);
    }
    if syntax_previous_word_is(buffer, start, b"struct")
        || syntax_previous_word_is(buffer, start, b"enum")
        || syntax_previous_word_is(buffer, start, b"trait")
        || syntax_previous_word_is(buffer, start, b"type")
    {
        return Some(SyntaxKind::Type);
    }
    if syntax_previous_word_is(buffer, start, b"const")
        || syntax_previous_word_is(buffer, start, b"static")
    {
        return Some(SyntaxKind::Constant);
    }
    let first = buffer_byte(buffer, start);
    let all_uppercase = (start..end).all(|position| {
        let byte = buffer_byte(buffer, position);
        !byte.is_ascii_alphabetic() || byte.is_ascii_uppercase()
    });
    if all_uppercase
        && (start..end).any(|position| buffer_byte(buffer, position).is_ascii_alphabetic())
    {
        return Some(SyntaxKind::Constant);
    }
    if first.is_ascii_uppercase() {
        return Some(SyntaxKind::Type);
    }
    if matches!(
        syntax_next_nonwhitespace(buffer, end, length),
        Some(b'(' | b'!')
    ) {
        return Some(SyntaxKind::Function);
    }
    None
}

fn syntax_keyword(buffer: &GapBuffer, language: SyntaxLanguage, start: usize, end: usize) -> bool {
    let keywords: &[&[u8]] = match language {
        SyntaxLanguage::Rust => &[
            b"as",
            b"async",
            b"await",
            b"break",
            b"const",
            b"continue",
            b"crate",
            b"dyn",
            b"else",
            b"enum",
            b"extern",
            b"false",
            b"fn",
            b"for",
            b"if",
            b"impl",
            b"in",
            b"let",
            b"loop",
            b"match",
            b"mod",
            b"move",
            b"mut",
            b"pub",
            b"ref",
            b"return",
            b"self",
            b"Self",
            b"static",
            b"struct",
            b"super",
            b"trait",
            b"true",
            b"type",
            b"unsafe",
            b"use",
            b"where",
            b"while",
        ],
        SyntaxLanguage::C | SyntaxLanguage::Cpp => &[
            b"alignas",
            b"auto",
            b"bool",
            b"break",
            b"case",
            b"catch",
            b"char",
            b"class",
            b"const",
            b"constexpr",
            b"continue",
            b"default",
            b"delete",
            b"do",
            b"double",
            b"else",
            b"enum",
            b"explicit",
            b"extern",
            b"false",
            b"float",
            b"for",
            b"friend",
            b"if",
            b"inline",
            b"int",
            b"long",
            b"namespace",
            b"new",
            b"nullptr",
            b"operator",
            b"private",
            b"protected",
            b"public",
            b"register",
            b"return",
            b"short",
            b"signed",
            b"sizeof",
            b"static",
            b"struct",
            b"switch",
            b"template",
            b"this",
            b"throw",
            b"true",
            b"try",
            b"typedef",
            b"typename",
            b"union",
            b"unsigned",
            b"using",
            b"virtual",
            b"void",
            b"volatile",
            b"while",
        ],
        SyntaxLanguage::Go => &[
            b"break",
            b"case",
            b"chan",
            b"const",
            b"continue",
            b"default",
            b"defer",
            b"else",
            b"fallthrough",
            b"for",
            b"func",
            b"go",
            b"goto",
            b"if",
            b"import",
            b"interface",
            b"map",
            b"package",
            b"range",
            b"return",
            b"select",
            b"struct",
            b"switch",
            b"type",
            b"var",
        ],
        SyntaxLanguage::Jai => &[
            b"break",
            b"case",
            b"cast",
            b"continue",
            b"defer",
            b"else",
            b"enum",
            b"for",
            b"if",
            b"inline",
            b"no_inline",
            b"null",
            b"return",
            b"struct",
            b"switch",
            b"true",
            b"false",
            b"union",
            b"while",
        ],
        SyntaxLanguage::Nim => &[
            b"and",
            b"as",
            b"block",
            b"break",
            b"case",
            b"concept",
            b"const",
            b"continue",
            b"converter",
            b"defer",
            b"discard",
            b"distinct",
            b"div",
            b"do",
            b"elif",
            b"else",
            b"end",
            b"enum",
            b"except",
            b"export",
            b"finally",
            b"for",
            b"from",
            b"func",
            b"if",
            b"import",
            b"in",
            b"include",
            b"interface",
            b"is",
            b"iterator",
            b"let",
            b"macro",
            b"method",
            b"mixin",
            b"mod",
            b"nil",
            b"not",
            b"object",
            b"of",
            b"or",
            b"out",
            b"proc",
            b"ptr",
            b"raise",
            b"ref",
            b"return",
            b"shl",
            b"shr",
            b"static",
            b"template",
            b"try",
            b"tuple",
            b"type",
            b"using",
            b"var",
            b"when",
            b"while",
            b"with",
            b"without",
            b"xor",
            b"yield",
        ],
        SyntaxLanguage::Odin => &[
            b"break",
            b"case",
            b"cast",
            b"context",
            b"continue",
            b"defer",
            b"distinct",
            b"do",
            b"else",
            b"enum",
            b"fallthrough",
            b"for",
            b"foreign",
            b"if",
            b"import",
            b"in",
            b"map",
            b"not_in",
            b"or_else",
            b"package",
            b"proc",
            b"return",
            b"struct",
            b"switch",
            b"transmute",
            b"typeid",
            b"union",
            b"when",
            b"where",
        ],
        SyntaxLanguage::Toml => &[b"false", b"true"],
        SyntaxLanguage::Shell => &[
            b"alias",
            b"bg",
            b"builtin",
            b"cd",
            b"command",
            b"dirs",
            b"disown",
            b"echo",
            b"eval",
            b"exec",
            b"export",
            b"false",
            b"fg",
            b"getopts",
            b"hash",
            b"jobs",
            b"local",
            b"popd",
            b"printf",
            b"pushd",
            b"pwd",
            b"read",
            b"readonly",
            b"set",
            b"shift",
            b"source",
            b"test",
            b"times",
            b"trap",
            b"true",
            b"typeset",
            b"ulimit",
            b"umask",
            b"unalias",
            b"unset",
            b"wait",
        ],
        SyntaxLanguage::None | SyntaxLanguage::Markdown => &[],
    };
    keywords
        .iter()
        .any(|keyword| syntax_range_equal(buffer, start, end, keyword))
}

fn syntax_starts_line_comment(language: SyntaxLanguage, byte: u8, next: Option<u8>) -> bool {
    match language {
        SyntaxLanguage::Toml | SyntaxLanguage::Nim | SyntaxLanguage::Shell => {
            byte == b'#' && next != Some(b'[')
        }
        SyntaxLanguage::Rust
        | SyntaxLanguage::C
        | SyntaxLanguage::Cpp
        | SyntaxLanguage::Go
        | SyntaxLanguage::Jai
        | SyntaxLanguage::Odin => byte == b'/' && next == Some(b'/'),
        SyntaxLanguage::None | SyntaxLanguage::Markdown => false,
    }
}

fn syntax_starts_block_comment(language: SyntaxLanguage, byte: u8, next: Option<u8>) -> bool {
    match language {
        SyntaxLanguage::Nim => byte == b'#' && next == Some(b'['),
        SyntaxLanguage::Rust
        | SyntaxLanguage::C
        | SyntaxLanguage::Cpp
        | SyntaxLanguage::Go
        | SyntaxLanguage::Jai
        | SyntaxLanguage::Odin => byte == b'/' && next == Some(b'*'),
        SyntaxLanguage::None
        | SyntaxLanguage::Toml
        | SyntaxLanguage::Markdown
        | SyntaxLanguage::Shell => false,
    }
}

fn syntax_starts_string(language: SyntaxLanguage, byte: u8) -> bool {
    match language {
        SyntaxLanguage::Go | SyntaxLanguage::Shell => matches!(byte, b'\'' | b'"' | b'`'),
        SyntaxLanguage::Toml | SyntaxLanguage::Nim => matches!(byte, b'\'' | b'"'),
        SyntaxLanguage::Rust
        | SyntaxLanguage::C
        | SyntaxLanguage::Cpp
        | SyntaxLanguage::Jai
        | SyntaxLanguage::Odin => matches!(byte, b'\'' | b'"'),
        SyntaxLanguage::None | SyntaxLanguage::Markdown => false,
    }
}

fn syntax_starts_attribute(
    language: SyntaxLanguage,
    line_start: bool,
    byte: u8,
    next: Option<u8>,
) -> bool {
    (language == SyntaxLanguage::Rust && byte == b'#' && next == Some(b'['))
        || (matches!(language, SyntaxLanguage::C | SyntaxLanguage::Cpp)
            && line_start
            && byte == b'#')
        || (language == SyntaxLanguage::Jai && byte == b'#')
        || (language == SyntaxLanguage::Odin && byte == b'@')
}

#[inline]
fn syntax_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn syntax_next_nonwhitespace(buffer: &GapBuffer, mut position: usize, length: usize) -> Option<u8> {
    while position < length {
        let byte = buffer_byte(buffer, position);
        if !byte.is_ascii_whitespace() {
            return Some(byte);
        }
        position += 1;
    }
    None
}

fn syntax_bytes_equal(buffer: &GapBuffer, start: usize, expected: &[u8]) -> bool {
    if start + expected.len() > buffer_len(buffer) {
        return false;
    }
    expected
        .iter()
        .enumerate()
        .all(|(offset, &byte)| buffer_byte(buffer, start + offset) == byte)
}

fn syntax_repeated_byte(buffer: &GapBuffer, start: usize, expected: u8, count: usize) -> bool {
    start + count <= buffer_len(buffer)
        && (start..start + count).all(|position| buffer_byte(buffer, position) == expected)
}

fn syntax_range_equal(buffer: &GapBuffer, start: usize, end: usize, expected: &[u8]) -> bool {
    end - start == expected.len()
        && expected
            .iter()
            .enumerate()
            .all(|(offset, &byte)| buffer_byte(buffer, start + offset) == byte)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::{buffer_from_bytes, buffer_insert};

    fn highlight(path: &str, source: &str) -> SyntaxHighlighting {
        let buffer = buffer_from_bytes(source.as_bytes());
        let mut highlighting = syntax_highlighting_empty();
        syntax_highlighting_set_path(&mut highlighting, Some(std::path::Path::new(path)));
        while !highlighting.complete {
            syntax_highlighting_step(&buffer, &mut highlighting, 16, std::time::Duration::MAX);
        }
        highlighting
    }

    #[test]
    fn extensions_select_requested_languages() {
        assert_eq!(
            syntax_language_from_path(Some(std::path::Path::new("a.toml"))),
            SyntaxLanguage::Toml
        );
        assert_eq!(
            syntax_language_from_path(Some(std::path::Path::new("a.md"))),
            SyntaxLanguage::Markdown
        );
        assert_eq!(
            syntax_language_from_path(Some(std::path::Path::new("a.rs"))),
            SyntaxLanguage::Rust
        );
        assert_eq!(
            syntax_language_from_path(Some(std::path::Path::new("a.c"))),
            SyntaxLanguage::C
        );
        assert_eq!(
            syntax_language_from_path(Some(std::path::Path::new("a.cpp"))),
            SyntaxLanguage::Cpp
        );
        assert_eq!(
            syntax_language_from_path(Some(std::path::Path::new("a.go"))),
            SyntaxLanguage::Go
        );
        assert_eq!(
            syntax_language_from_path(Some(std::path::Path::new("a.jai"))),
            SyntaxLanguage::Jai
        );
        assert_eq!(
            syntax_language_from_path(Some(std::path::Path::new("a.nim"))),
            SyntaxLanguage::Nim
        );
        assert_eq!(
            syntax_language_from_path(Some(std::path::Path::new("a.odin"))),
            SyntaxLanguage::Odin
        );
        assert_eq!(
            syntax_language_from_path(Some(std::path::Path::new(".zshrc"))),
            SyntaxLanguage::Shell
        );
        assert_eq!(
            syntax_language_from_path(Some(std::path::Path::new("build.sh"))),
            SyntaxLanguage::Shell
        );
    }

    #[test]
    fn shell_scanner_highlights_control_strings_variables_and_comments() {
        let highlighting = highlight(
            ".zshrc",
            "if [[ -n \"$HOME\" ]]; then\n  export PATH=\"$HOME/bin:$PATH\" # tools\nfi",
        );
        assert!(
            highlighting
                .spans
                .iter()
                .any(|span| span.kind == SyntaxKind::Control)
        );
        assert!(
            highlighting
                .spans
                .iter()
                .any(|span| span.kind == SyntaxKind::String)
        );
        assert!(
            highlighting
                .spans
                .iter()
                .any(|span| span.kind == SyntaxKind::Comment)
        );
    }

    #[test]
    fn rust_scanner_resumes_and_classifies_tokens() {
        let highlighting = highlight(
            "main.rs",
            "pub struct Thing { value: i32 } // note\nfn make() -> Thing { Thing { value: 42 } }",
        );
        assert!(
            highlighting
                .spans
                .iter()
                .any(|span| span.kind == SyntaxKind::Comment)
        );
        assert!(
            highlighting
                .spans
                .iter()
                .any(|span| span.kind == SyntaxKind::Keyword)
        );
        assert!(
            highlighting
                .spans
                .iter()
                .any(|span| span.kind == SyntaxKind::Type)
        );
        assert!(
            highlighting
                .spans
                .iter()
                .any(|span| span.kind == SyntaxKind::Number)
        );
    }

    #[test]
    fn rust_semantics_and_comment_annotations_are_distinct() {
        let highlighting = highlight(
            "main.rs",
            "// NOTE(Erik): fast path\nconst LIMIT: usize = 4;\nfn run() { if LIMIT > 0 {} }",
        );
        for kind in [
            SyntaxKind::CommentNote,
            SyntaxKind::Declaration,
            SyntaxKind::Constant,
            SyntaxKind::Function,
            SyntaxKind::Control,
            SyntaxKind::Operator,
            SyntaxKind::Punctuation,
        ] {
            assert!(
                highlighting.spans.iter().any(|span| span.kind == kind),
                "missing {kind:?}"
            );
        }
    }

    #[test]
    fn rust_lifetimes_are_not_character_strings() {
        let highlighting = highlight(
            "lib.rs",
            "pub type RapidHasher<'s> = rapidhash::fast::RapidHasher<'s>; let byte = 's';",
        );
        assert_eq!(
            highlighting
                .spans
                .iter()
                .filter(|span| span.kind == SyntaxKind::Lifetime)
                .count(),
            2
        );
        assert_eq!(
            highlighting
                .spans
                .iter()
                .filter(|span| span.kind == SyntaxKind::String)
                .count(),
            1
        );
    }

    #[test]
    fn comment_annotations_use_severity_kinds() {
        let highlighting = highlight(
            "lib.rs",
            "// NOTE(Erik): context\n// WARNING: careful\n// HACK: temporary\n// FIXME: broken\n// SILLY_RUST(Erik): compiler workaround\n",
        );
        for kind in [
            SyntaxKind::CommentNote,
            SyntaxKind::CommentWarning,
            SyntaxKind::CommentError,
            SyntaxKind::CommentAnnotation,
        ] {
            assert!(
                highlighting.spans.iter().any(|span| span.kind == kind),
                "missing {kind:?}"
            );
        }
    }

    #[test]
    fn invalidation_keeps_published_spans_until_rebuild_finishes() {
        let source = "fn main() { return; }";
        let buffer = buffer_from_bytes(source.as_bytes());
        let mut highlighting = highlight("main.rs", source);
        let published = highlighting.spans.clone();
        syntax_highlighting_invalidate(&mut highlighting);
        assert_eq!(syntax_highlighting_spans(&highlighting), published);
        syntax_highlighting_step(
            &buffer,
            &mut highlighting,
            usize::MAX,
            std::time::Duration::MAX,
        );
        assert!(highlighting.complete);
        assert!(highlighting.previous_spans.is_empty());
    }

    #[test]
    fn edit_invalidation_resumes_from_the_nearest_lexer_checkpoint() {
        let original = "fn first() {\n    let text = \"one\";\n}\nfn second() {\n    return;\n}\n";
        let mut buffer = buffer_from_bytes(original.as_bytes());
        let mut highlighting = highlight("main.rs", original);
        let edit = original.find("return").unwrap();
        let restart = original[..edit].rfind('\n').unwrap() + 1;
        syntax_highlighting_invalidate_edits(&mut highlighting, &[(edit, edit, 4)]);
        buffer_insert(&mut buffer, edit, b"loop");

        assert_eq!(highlighting.position, restart);
        assert!(highlighting.position > 0);
        assert!(!highlighting.spans.is_empty());
        while !highlighting.complete {
            syntax_highlighting_step(&buffer, &mut highlighting, 16, std::time::Duration::MAX);
        }

        let edited = original.replacen("return", "loopreturn", 1);
        let rebuilt = highlight("main.rs", &edited);
        assert_eq!(highlighting.spans, rebuilt.spans);
    }

    #[test]
    fn toml_and_markdown_have_structural_highlights() {
        let toml = highlight(
            "bed.toml",
            "[editor.display]\ntheme = \"kanagawa\"\nenabled = true\nwhen = 1979-05-27T07:32:00Z # color\n",
        );
        for kind in [
            SyntaxKind::Type,
            SyntaxKind::Punctuation,
            SyntaxKind::Attribute,
            SyntaxKind::Constant,
            SyntaxKind::String,
            SyntaxKind::Comment,
        ] {
            assert!(
                toml.spans.iter().any(|span| span.kind == kind),
                "missing TOML {kind:?}"
            );
        }

        let markdown = highlight("README.md", "# Heading\n`code` and [link](target)\n");
        assert!(
            markdown
                .spans
                .iter()
                .filter(|span| span.kind == SyntaxKind::Markup)
                .count()
                >= 2
        );
    }
}
