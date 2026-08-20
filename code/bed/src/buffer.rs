use core::alloc::Allocator;
use std::io::Write;

const INITIAL_GAP_BYTES: usize = 1024;

pub struct GapBuffer {
    pub bytes: Vec<u8>,
    pub gap_start: usize,
    pub gap_end: usize,
    pub line_starts: Vec<u32>,
}

pub fn buffer_from_bytes(source: &[u8]) -> GapBuffer {
    assert!(u32::try_from(source.len()).is_ok());
    let mut bytes = vec![0; source.len() + INITIAL_GAP_BYTES];
    bytes[..source.len()].copy_from_slice(source);
    let mut line_starts =
        Vec::with_capacity(source.iter().filter(|&&byte| byte == b'\n').count() + 1);
    line_starts.push(0);
    line_starts.extend(
        source
            .iter()
            .enumerate()
            .filter_map(|(position, &byte)| (byte == b'\n').then_some(position as u32 + 1)),
    );
    GapBuffer {
        bytes,
        gap_start: source.len(),
        gap_end: source.len() + INITIAL_GAP_BYTES,
        line_starts,
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

#[inline]
pub fn buffer_slices(buffer: &GapBuffer) -> (&[u8], &[u8]) {
    (
        &buffer.bytes[..buffer.gap_start],
        &buffer.bytes[buffer.gap_end..],
    )
}

pub fn buffer_write(buffer: &GapBuffer, output: &mut impl Write) -> std::io::Result<()> {
    let (before_gap, after_gap) = buffer_slices(buffer);
    if let Err(error) = output.write_all(before_gap) {
        return Err(error);
    }
    output.write_all(after_gap)
}

pub fn buffer_append_range(
    buffer: &GapBuffer,
    start: usize,
    end: usize,
    result: &mut Vec<u8, impl Allocator>,
) {
    assert!(start <= end && end <= buffer_len(buffer));
    result.reserve(end - start);
    let (before_gap, after_gap) = buffer_slices(buffer);
    if start < buffer.gap_start {
        result.extend_from_slice(&before_gap[start..end.min(buffer.gap_start)]);
    }
    if end > buffer.gap_start {
        result.extend_from_slice(
            &after_gap[start.max(buffer.gap_start) - buffer.gap_start..end - buffer.gap_start],
        );
    }
}

#[inline]
pub fn buffer_line_count(buffer: &GapBuffer) -> usize {
    buffer.line_starts.len()
}

pub fn buffer_insert(buffer: &mut GapBuffer, position: usize, inserted: &[u8]) {
    profiling::function_scope!();
    assert!(position <= buffer_len(buffer));
    assert!(u32::try_from(buffer_len(buffer).saturating_add(inserted.len())).is_ok());
    buffer_line_index_insert(buffer, position, inserted);
    buffer_move_gap(buffer, position);
    buffer_reserve_gap(buffer, inserted.len());

    let end = buffer.gap_start + inserted.len();
    buffer.bytes[buffer.gap_start..end].copy_from_slice(inserted);
    buffer.gap_start = end;
}

pub fn buffer_delete(buffer: &mut GapBuffer, start: usize, end: usize) {
    profiling::function_scope!();
    assert!(start <= end && end <= buffer_len(buffer));
    buffer_line_index_delete(buffer, start, end);
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

pub fn buffer_decode_char(buffer: &GapBuffer, position: usize) -> (char, usize) {
    let length = buffer_len(buffer);
    if position >= length {
        return ('\0', length);
    }
    let next = buffer_next_char(buffer, position);
    let byte_length = next - position;
    if byte_length == 1 {
        let byte = buffer_byte(buffer, position);
        return if byte.is_ascii() {
            (byte as char, next)
        } else {
            ('\u{fffd}', next)
        };
    }
    let mut bytes = [0; 4];
    let decoded_length = byte_length.min(bytes.len());
    for (offset, byte) in bytes[..decoded_length].iter_mut().enumerate() {
        *byte = buffer_byte(buffer, position + offset);
    }
    let character = std::str::from_utf8(&bytes[..decoded_length])
        .ok()
        .and_then(|text| text.chars().next())
        .unwrap_or('\u{fffd}');
    (character, next)
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
    let position = position.min(buffer_len(buffer)) as u32;
    let line = buffer
        .line_starts
        .partition_point(|&line_start| line_start <= position)
        .saturating_sub(1);
    buffer.line_starts[line] as usize
}

pub fn buffer_line_end(buffer: &GapBuffer, position: usize) -> usize {
    let position = position.min(buffer_len(buffer)) as u32;
    let line = buffer
        .line_starts
        .partition_point(|&line_start| line_start <= position)
        .saturating_sub(1);
    buffer
        .line_starts
        .get(line + 1)
        .map_or(buffer_len(buffer), |&next_line| next_line as usize - 1)
}

pub fn buffer_line_and_column(buffer: &GapBuffer, position: usize) -> (usize, usize) {
    let position = position.min(buffer_len(buffer));
    let line = buffer
        .line_starts
        .partition_point(|&line_start| line_start as usize <= position)
        .saturating_sub(1);
    (line, position - buffer.line_starts[line] as usize)
}

pub fn buffer_position_at_line_column(
    buffer: &GapBuffer,
    target_line: usize,
    column: usize,
) -> usize {
    let position = buffer
        .line_starts
        .get(target_line)
        .map_or(buffer_len(buffer), |&position| position as usize);
    let end = buffer_line_end(buffer, position);
    (position + column).min(end)
}

fn buffer_line_index_insert(buffer: &mut GapBuffer, position: usize, inserted: &[u8]) {
    profiling::function_scope!();
    if inserted.is_empty() {
        return;
    }
    let inserted_line_count = inserted.iter().filter(|&&byte| byte == b'\n').count();
    let insertion_line = buffer
        .line_starts
        .partition_point(|&line_start| line_start as usize <= position);
    let previous_length = buffer.line_starts.len();
    buffer.line_starts.reserve(inserted_line_count);
    buffer
        .line_starts
        .resize(previous_length + inserted_line_count, 0);
    buffer.line_starts.copy_within(
        insertion_line..previous_length,
        insertion_line + inserted_line_count,
    );
    for line_start in &mut buffer.line_starts[insertion_line + inserted_line_count..] {
        *line_start += inserted.len() as u32;
    }
    let mut line = insertion_line;
    for (offset, &byte) in inserted.iter().enumerate() {
        if byte == b'\n' {
            buffer.line_starts[line] = (position + offset + 1) as u32;
            line += 1;
        }
    }
}

fn buffer_line_index_delete(buffer: &mut GapBuffer, start: usize, end: usize) {
    profiling::function_scope!();
    if start == end {
        return;
    }
    let first_removed = buffer
        .line_starts
        .partition_point(|&line_start| line_start as usize <= start);
    let after_removed = buffer
        .line_starts
        .partition_point(|&line_start| line_start as usize <= end);
    buffer.line_starts.drain(first_removed..after_removed);
    for line_start in &mut buffer.line_starts[first_removed..] {
        *line_start -= (end - start) as u32;
    }
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
        assert_eq!(buffer_decode_char(&buffer, 1), ('å', 3));
        assert_eq!(buffer_decode_char(&buffer, 3), ('🦀', 7));
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

    #[test]
    fn line_index_tracks_insertions_deletions_and_final_empty_lines() {
        let mut buffer = buffer_from_bytes(b"one\ntwo\n");
        assert_eq!(buffer.line_starts, [0, 4, 8]);
        assert_eq!(buffer_line_count(&buffer), 3);
        assert_eq!(buffer_line_and_column(&buffer, 5), (1, 1));
        assert_eq!(buffer_position_at_line_column(&buffer, 2, 99), 8);

        buffer_insert(&mut buffer, 4, b"new\nlines\n");
        assert_eq!(contents(&buffer), b"one\nnew\nlines\ntwo\n");
        assert_eq!(buffer.line_starts, [0, 4, 8, 14, 18]);
        assert_eq!(buffer_line_and_column(&buffer, 16), (3, 2));

        buffer_delete(&mut buffer, 3, 14);
        assert_eq!(contents(&buffer), b"onetwo\n");
        assert_eq!(buffer.line_starts, [0, 7]);
        assert_eq!(buffer_line_end(&buffer, 2), 6);
        assert_eq!(buffer_line_start(&buffer, 7), 7);
    }

    #[test]
    fn range_append_copies_across_the_gap_in_two_slices() {
        let mut buffer = buffer_from_bytes(b"abcdef");
        buffer_insert(&mut buffer, 3, b"XYZ");
        let mut result = Vec::new();
        buffer_append_range(&buffer, 1, 8, &mut result);
        assert_eq!(result, b"bcXYZde");
    }

    #[test]
    #[ignore = "manual release-mode line lookup measurement"]
    fn measure_indexed_line_queries_on_editor_source() {
        let buffer = buffer_from_bytes(include_bytes!("editor.rs"));
        let indexed_iterations = 1_000_000usize;
        let started = std::time::Instant::now();
        for iteration in 0..indexed_iterations {
            let position = iteration.wrapping_mul(97) % buffer_len(&buffer).max(1);
            std::hint::black_box(buffer_line_count(std::hint::black_box(&buffer)));
            std::hint::black_box(buffer_line_and_column(
                std::hint::black_box(&buffer),
                position,
            ));
        }
        let indexed = started.elapsed();

        let scan_iterations = 100usize;
        let started = std::time::Instant::now();
        for _ in 0..scan_iterations {
            let (before_gap, after_gap) = buffer_slices(std::hint::black_box(&buffer));
            let lines = before_gap
                .iter()
                .chain(after_gap)
                .filter(|&&byte| byte == b'\n')
                .count()
                + 1;
            std::hint::black_box(lines);
        }
        let scanned = started.elapsed();
        eprintln!(
            "indexed count+position: {:?}/{} calls; full line scan: {:?}/{} calls",
            indexed, indexed_iterations, scanned, scan_iterations
        );
    }
}
