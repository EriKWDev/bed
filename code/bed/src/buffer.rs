use core::alloc::Allocator;
use std::io::Write;

const INITIAL_GAP_BYTES: usize = 1024;

pub struct GapBuffer {
    pub bytes: Vec<u8>,
    pub gap_start: usize,
    pub gap_end: usize,
}

pub fn buffer_from_bytes(source: &[u8]) -> GapBuffer {
    let mut bytes = vec![0; source.len() + INITIAL_GAP_BYTES];
    bytes[..source.len()].copy_from_slice(source);
    GapBuffer {
        bytes,
        gap_start: source.len(),
        gap_end: source.len() + INITIAL_GAP_BYTES,
    }
}

#[inline]
pub fn buffer_len(buffer: &GapBuffer) -> usize {
    buffer.bytes.len() - (buffer.gap_end - buffer.gap_start)
}

#[inline]
pub fn buffer_byte(buffer: &GapBuffer, position: usize) -> u8 {
    assert!(position < buffer_len(buffer));
    if position < buffer.gap_start {
        buffer.bytes[position]
    } else {
        buffer.bytes[position + buffer.gap_end - buffer.gap_start]
    }
}

pub fn buffer_write(buffer: &GapBuffer, output: &mut impl Write) -> std::io::Result<()> {
    if let Err(error) = output.write_all(&buffer.bytes[..buffer.gap_start]) {
        return Err(error);
    }
    output.write_all(&buffer.bytes[buffer.gap_end..])
}

pub fn buffer_append_range(
    buffer: &GapBuffer,
    start: usize,
    end: usize,
    result: &mut Vec<u8, impl Allocator>,
) {
    assert!(start <= end && end <= buffer_len(buffer));
    result.reserve(end - start);
    for position in start..end {
        result.push(buffer_byte(buffer, position));
    }
}

pub fn buffer_line_count(buffer: &GapBuffer) -> usize {
    let mut lines = 1;
    for position in 0..buffer_len(buffer) {
        lines += usize::from(buffer_byte(buffer, position) == b'\n');
    }
    lines
}

pub fn buffer_insert(buffer: &mut GapBuffer, position: usize, inserted: &[u8]) {
    profiling::function_scope!();
    assert!(position <= buffer_len(buffer));
    buffer_move_gap(buffer, position);
    buffer_reserve_gap(buffer, inserted.len());

    let end = buffer.gap_start + inserted.len();
    buffer.bytes[buffer.gap_start..end].copy_from_slice(inserted);
    buffer.gap_start = end;
}

pub fn buffer_delete(buffer: &mut GapBuffer, start: usize, end: usize) {
    profiling::function_scope!();
    assert!(start <= end && end <= buffer_len(buffer));
    buffer_move_gap(buffer, start);
    buffer.gap_end += end - start;
}

pub fn buffer_next_char(buffer: &GapBuffer, position: usize) -> usize {
    let length = buffer_len(buffer);
    let mut next = (position + 1).min(length);
    while next < length && buffer_byte(buffer, next) & 0b1100_0000 == 0b1000_0000 {
        next += 1;
    }
    next
}

pub fn buffer_previous_char(buffer: &GapBuffer, position: usize) -> usize {
    let mut previous = position.saturating_sub(1);
    while previous > 0 && buffer_byte(buffer, previous) & 0b1100_0000 == 0b1000_0000 {
        previous -= 1;
    }
    previous
}

pub fn buffer_next_word_start(buffer: &GapBuffer, position: usize) -> usize {
    let length = buffer_len(buffer);
    let mut position = position.min(length);
    if position >= length {
        return position;
    }
    let category = byte_word_category(buffer_byte(buffer, position));
    position = buffer_next_char(buffer, position);
    while position < length && byte_word_category(buffer_byte(buffer, position)) == category {
        position = buffer_next_char(buffer, position);
    }
    while position < length
        && byte_word_category(buffer_byte(buffer, position)) == WordCategory::Whitespace
    {
        position = buffer_next_char(buffer, position);
    }
    position
}

pub fn buffer_previous_word_start(buffer: &GapBuffer, position: usize) -> usize {
    let mut position = position.min(buffer_len(buffer));
    if position == 0 {
        return 0;
    }
    position = buffer_previous_char(buffer, position);
    while position > 0
        && byte_word_category(buffer_byte(buffer, position)) == WordCategory::Whitespace
    {
        position = buffer_previous_char(buffer, position);
    }
    let category = byte_word_category(buffer_byte(buffer, position));
    while position > 0 {
        let previous = buffer_previous_char(buffer, position);
        if byte_word_category(buffer_byte(buffer, previous)) != category {
            break;
        }
        position = previous;
    }
    position
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WordCategory {
    Word,
    Whitespace,
    EndOfLine,
    Punctuation,
}

fn byte_word_category(byte: u8) -> WordCategory {
    if matches!(byte, b'\n' | b'\r') {
        WordCategory::EndOfLine
    } else if byte.is_ascii_whitespace() {
        WordCategory::Whitespace
    } else if byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80 {
        WordCategory::Word
    } else {
        WordCategory::Punctuation
    }
}

pub fn buffer_line_start(buffer: &GapBuffer, position: usize) -> usize {
    let mut start = position.min(buffer_len(buffer));
    while start > 0 && buffer_byte(buffer, start - 1) != b'\n' {
        start -= 1;
    }
    start
}

pub fn buffer_line_end(buffer: &GapBuffer, position: usize) -> usize {
    let length = buffer_len(buffer);
    let mut end = position.min(length);
    while end < length && buffer_byte(buffer, end) != b'\n' {
        end += 1;
    }
    end
}

pub fn buffer_line_and_column(buffer: &GapBuffer, position: usize) -> (usize, usize) {
    let position = position.min(buffer_len(buffer));
    let mut line = 0;
    let mut line_start = 0;
    for byte_position in 0..position {
        if buffer_byte(buffer, byte_position) == b'\n' {
            line += 1;
            line_start = byte_position + 1;
        }
    }
    (line, position - line_start)
}

pub fn buffer_position_at_line_column(
    buffer: &GapBuffer,
    target_line: usize,
    column: usize,
) -> usize {
    let length = buffer_len(buffer);
    let mut line = 0;
    let mut position = 0;
    while position < length && line < target_line {
        if buffer_byte(buffer, position) == b'\n' {
            line += 1;
        }
        position += 1;
    }

    let end = buffer_line_end(buffer, position);
    (position + column).min(end)
}

fn buffer_move_gap(buffer: &mut GapBuffer, position: usize) {
    profiling::function_scope!();
    if position < buffer.gap_start {
        let moved = buffer.gap_start - position;
        buffer
            .bytes
            .copy_within(position..buffer.gap_start, buffer.gap_end - moved);
        buffer.gap_start -= moved;
        buffer.gap_end -= moved;
    } else if position > buffer.gap_start {
        let moved = position - buffer.gap_start;
        buffer
            .bytes
            .copy_within(buffer.gap_end..buffer.gap_end + moved, buffer.gap_start);
        buffer.gap_start += moved;
        buffer.gap_end += moved;
    }
}

fn buffer_reserve_gap(buffer: &mut GapBuffer, needed: usize) {
    let gap_length = buffer.gap_end - buffer.gap_start;
    if gap_length >= needed {
        return;
    }

    let old_length = buffer_len(buffer);
    let new_gap_length = needed.max(buffer.bytes.len().max(INITIAL_GAP_BYTES));
    let mut bytes = Vec::with_capacity(old_length + new_gap_length);
    bytes.extend_from_slice(&buffer.bytes[..buffer.gap_start]);
    bytes.resize(buffer.gap_start + new_gap_length, 0);
    bytes.extend_from_slice(&buffer.bytes[buffer.gap_end..]);
    buffer.bytes = bytes;
    buffer.gap_end = buffer.gap_start + new_gap_length;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contents(buffer: &GapBuffer) -> Vec<u8> {
        let mut result = Vec::new();
        buffer_write(buffer, &mut result).unwrap();
        result
    }

    #[test]
    fn insertion_and_deletion_move_the_gap_without_changing_other_bytes() {
        let mut buffer = buffer_from_bytes(b"hello world");
        buffer_insert(&mut buffer, 5, b", brave");
        assert_eq!(contents(&buffer), b"hello, brave world");

        buffer_delete(&mut buffer, 5, 12);
        assert_eq!(contents(&buffer), b"hello world");
    }

    #[test]
    fn utf8_motions_stay_on_codepoint_boundaries() {
        let buffer = buffer_from_bytes("aå🦀z".as_bytes());
        let mut position = 0;
        let mut positions = Vec::new();
        while position < buffer_len(&buffer) {
            positions.push(position);
            position = buffer_next_char(&buffer, position);
        }
        assert_eq!(positions, [0, 1, 3, 7]);
        assert_eq!(buffer_previous_char(&buffer, 7), 3);
    }

    #[test]
    fn word_motions_stop_at_helix_punctuation_boundaries() {
        let buffer = buffer_from_bytes(b"alpha({beta}) gamma");
        assert_eq!(buffer_next_word_start(&buffer, 0), 5);
        assert_eq!(buffer_next_word_start(&buffer, 5), 7);
        assert_eq!(buffer_next_word_start(&buffer, 7), 11);
        assert_eq!(buffer_previous_word_start(&buffer, 13), 11);
        assert_eq!(buffer_previous_word_start(&buffer, 11), 7);
    }
}
