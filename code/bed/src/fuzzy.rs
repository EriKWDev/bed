use core::alloc::Allocator;

pub const FUZZY_NO_MATCH: i32 = i32::MIN / 2;
pub const FUZZY_SCORE_MATCH: i32 = 16;
pub const FUZZY_SCORE_GAP_START: i32 = -3;
pub const FUZZY_SCORE_GAP_EXTENSION: i32 = -1;
pub const FUZZY_BONUS_BOUNDARY: i32 = 8;
pub const FUZZY_BONUS_CONSECUTIVE: i32 = 4;
pub const FUZZY_TRANSPOSITION_PENALTY: i32 = 8;
pub const FUZZY_FILE_NAME_LENGTH_PENALTY: i32 = 2;
pub const FUZZY_PARALLEL_ITEMS: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FuzzyMatch {
    pub item: usize,
    pub score: i32,
}

#[inline]
pub fn fuzzy_byte_matches(query_byte: u8, candidate_byte: u8) -> bool {
    if query_byte.is_ascii_uppercase() {
        candidate_byte == query_byte
    } else {
        candidate_byte.eq_ignore_ascii_case(&query_byte)
    }
}

pub fn fuzzy_rank(query: &str, items: &[&str], matches: &mut Vec<FuzzyMatch, impl Allocator>) {
    profiling::function_scope!();
    matches.clear();
    if query.is_empty() {
        matches.extend((0..items.len()).map(|item| FuzzyMatch { item, score: 0 }));
        return;
    }

    let temp = idno_std::mem().scratch().temp();
    let query = query.as_bytes();
    let mut item_scores = temp.vec(items.len());
    item_scores.resize(items.len(), FUZZY_NO_MATCH);
    if items.len() >= FUZZY_PARALLEL_ITEMS {
        profiling::scope!("score fuzzy paths (parallel)");
        use micropool::iter::*;
        (items.par_iter(), item_scores.par_iter_mut())
            .zip_eq()
            .with_thread_pool(idno_std::threads().split_by_threads())
            .for_each(|(candidate, item_score)| {
                let worker_temp = idno_std::mem().scratch().temp();
                let mut alignment_scratch = worker_temp.vec(query.len() * 6);
                *item_score = fuzzy_score_path(query, candidate.as_bytes(), &mut alignment_scratch);
            });
    } else {
        profiling::scope!("score fuzzy paths");
        let mut alignment_scratch = temp.vec(query.len() * 6);
        for (candidate, item_score) in items.iter().zip(&mut item_scores) {
            *item_score = fuzzy_score_path(query, candidate.as_bytes(), &mut alignment_scratch);
        }
    }
    for (item, &score) in item_scores.iter().enumerate() {
        if score != FUZZY_NO_MATCH {
            matches.push(FuzzyMatch { item, score });
        }
    }
    {
        profiling::scope!("sort fuzzy matches");
        matches.sort_unstable_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| items[left.item].len().cmp(&items[right.item].len()))
                .then_with(|| items[left.item].cmp(items[right.item]))
        });
    }
}

fn fuzzy_score_path(
    query: &[u8],
    path: &[u8],
    alignment_scratch: &mut Vec<i32, impl Allocator>,
) -> i32 {
    profiling::function_scope!();
    let mut score = fuzzy_score_alignment(query, path, alignment_scratch);
    if score == FUZZY_NO_MATCH {
        return FUZZY_NO_MATCH;
    }
    let mut final_segment_score = FUZZY_NO_MATCH;
    let mut final_segment_length = 0;
    for segment in path.split(|byte| matches!(byte, b'/' | b'\\')) {
        if segment.is_empty() {
            continue;
        }
        let segment_score = fuzzy_score_alignment(query, segment, alignment_scratch);
        if segment_score != FUZZY_NO_MATCH {
            score += segment_score;
        }
        final_segment_score = segment_score;
        final_segment_length = segment.len();
    }
    if final_segment_score != FUZZY_NO_MATCH {
        score += final_segment_score;
    }
    score -= final_segment_length.min(i32::MAX as usize) as i32 * FUZZY_FILE_NAME_LENGTH_PENALTY;
    score
}

fn fuzzy_score_alignment(
    query: &[u8],
    candidate: &[u8],
    alignment_scratch: &mut Vec<i32, impl Allocator>,
) -> i32 {
    profiling::function_scope!();
    let query_length = query.len();
    alignment_scratch.clear();
    alignment_scratch.resize(query_length * 6, FUZZY_NO_MATCH);
    let best_offset = 0;
    let ending_offset = query_length;
    let transposition_best_offset = query_length * 2;
    let transposition_ending_offset = query_length * 3;
    let best_position_offset = query_length * 4;
    let transposition_best_position_offset = query_length * 5;

    let mut result = FUZZY_NO_MATCH;
    for (position, &candidate_byte) in candidate.iter().enumerate() {
        for query_position in (0..query_length).rev() {
            let transposition_adjacent =
                alignment_scratch[transposition_ending_offset + query_position];
            alignment_scratch[ending_offset + query_position] = FUZZY_NO_MATCH;
            alignment_scratch[transposition_ending_offset + query_position] = FUZZY_NO_MATCH;

            if query_position + 1 < query_length
                && fuzzy_byte_matches(query[query_position], candidate_byte)
            {
                let byte_score = fuzzy_byte_score(candidate, position, query[query_position]);
                let transposition_best =
                    alignment_scratch[transposition_best_offset + query_position];
                if transposition_best != FUZZY_NO_MATCH {
                    let previous_position = alignment_scratch
                        [transposition_best_position_offset + query_position]
                        as usize;
                    let completed = fuzzy_extend_score(
                        transposition_best,
                        previous_position,
                        transposition_adjacent,
                        byte_score,
                        position,
                    ) - FUZZY_TRANSPOSITION_PENALTY;
                    let completed_position = query_position + 1;
                    alignment_scratch[ending_offset + completed_position] =
                        alignment_scratch[ending_offset + completed_position].max(completed);
                    if completed > alignment_scratch[best_offset + completed_position] {
                        alignment_scratch[best_offset + completed_position] = completed;
                        alignment_scratch[best_position_offset + completed_position] =
                            position as i32;
                    }
                    if completed_position + 1 == query_length {
                        let trailing_penalty = (candidate.len() - position - 1).min(255) as i32 / 8;
                        result = result.max(completed - trailing_penalty);
                    }
                }
            }

            let query_byte = query[query_position];
            if fuzzy_byte_matches(query_byte, candidate_byte) {
                let byte_score = fuzzy_byte_score(candidate, position, query_byte);
                let score = if query_position == 0 {
                    fuzzy_start_score(byte_score, position)
                } else {
                    let previous_best = alignment_scratch[best_offset + query_position - 1];
                    let adjacent = alignment_scratch[ending_offset + query_position - 1];
                    if previous_best == FUZZY_NO_MATCH {
                        FUZZY_NO_MATCH
                    } else {
                        fuzzy_extend_score(
                            previous_best,
                            alignment_scratch[best_position_offset + query_position - 1] as usize,
                            adjacent,
                            byte_score,
                            position,
                        )
                    }
                };
                if score != FUZZY_NO_MATCH {
                    alignment_scratch[ending_offset + query_position] =
                        alignment_scratch[ending_offset + query_position].max(score);
                    if score > alignment_scratch[best_offset + query_position] {
                        alignment_scratch[best_offset + query_position] = score;
                        alignment_scratch[best_position_offset + query_position] = position as i32;
                    }
                    if query_position + 1 == query_length {
                        let trailing_penalty = (candidate.len() - position - 1).min(255) as i32 / 8;
                        result = result.max(score - trailing_penalty);
                    }
                }
            }

            if query_position + 1 < query_length
                && query[query_position].is_ascii()
                && query[query_position + 1].is_ascii()
                && !query[query_position].eq_ignore_ascii_case(&query[query_position + 1])
                && fuzzy_byte_matches(query[query_position + 1], candidate_byte)
            {
                let byte_score = fuzzy_byte_score(candidate, position, query[query_position + 1]);
                let transposition = if query_position == 0 {
                    fuzzy_start_score(byte_score, position)
                } else {
                    let previous_best = alignment_scratch[best_offset + query_position - 1];
                    let adjacent = alignment_scratch[ending_offset + query_position - 1];
                    if previous_best == FUZZY_NO_MATCH {
                        FUZZY_NO_MATCH
                    } else {
                        fuzzy_extend_score(
                            previous_best,
                            alignment_scratch[best_position_offset + query_position - 1] as usize,
                            adjacent,
                            byte_score,
                            position,
                        )
                    }
                };
                if transposition != FUZZY_NO_MATCH {
                    alignment_scratch[transposition_ending_offset + query_position] = transposition;
                    if transposition > alignment_scratch[transposition_best_offset + query_position]
                    {
                        alignment_scratch[transposition_best_offset + query_position] =
                            transposition;
                        alignment_scratch[transposition_best_position_offset + query_position] =
                            position as i32;
                    }
                }
            }
        }
    }
    result
}

#[inline]
fn fuzzy_start_score(byte_score: i32, position: usize) -> i32 {
    if position == 0 {
        byte_score
    } else {
        byte_score
            + FUZZY_SCORE_GAP_START
            + position.min(i32::MAX as usize) as i32 * FUZZY_SCORE_GAP_EXTENSION
    }
}

#[inline]
fn fuzzy_extend_score(
    previous_best: i32,
    previous_position: usize,
    previous_adjacent: i32,
    byte_score: i32,
    position: usize,
) -> i32 {
    let gap = position - previous_position - 1;
    let gap_score = if gap == 0 {
        FUZZY_BONUS_CONSECUTIVE
    } else {
        FUZZY_SCORE_GAP_START + gap.min(i32::MAX as usize) as i32 * FUZZY_SCORE_GAP_EXTENSION
    };
    (previous_best + byte_score + gap_score)
        .max(previous_adjacent + byte_score + FUZZY_BONUS_CONSECUTIVE)
}

#[inline]
fn fuzzy_byte_score(candidate: &[u8], position: usize, query_byte: u8) -> i32 {
    let mut score = FUZZY_SCORE_MATCH;
    if position == 0 || matches_path_boundary(candidate[position - 1]) {
        score += FUZZY_BONUS_BOUNDARY;
    }
    if candidate[position] == query_byte {
        score += 1;
    }
    score
}

#[inline]
fn matches_path_boundary(byte: u8) -> bool {
    matches!(byte, b'/' | b'\\' | b'_' | b'-' | b'.' | b' ')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adjacent_and_boundary_matches_rank_first() {
        let items = ["source/editor.rs", "docs/reconsider.md"];
        let mut matches = Vec::new();
        fuzzy_rank("ed", &items, &mut matches);
        assert_eq!(matches[0].item, 0);

        let items = ["code/bed/src/main.rs", "code/bed/src/editor.rs"];
        fuzzy_rank("edi", &items, &mut matches);
        assert_eq!(matches[0].item, 1);
    }

    #[test]
    fn missing_characters_do_not_match() {
        let mut matches = Vec::new();
        fuzzy_rank("xyz", &["example.rs"], &mut matches);
        assert!(matches.is_empty());
    }

    #[test]
    fn lowercase_queries_match_both_cases() {
        let items = ["allocator", "ALLOCATOR"];
        let mut matches = Vec::new();
        fuzzy_rank("allocator", &items, &mut matches);
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn uppercase_query_bytes_only_match_uppercase_candidates() {
        let items = ["allocator", "ALLOCATOR", "Allocator"];
        let mut matches = Vec::new();
        fuzzy_rank("ALLOC", &items, &mut matches);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].item, 1);
    }

    #[test]
    fn adjacent_query_typo_matches_file_name() {
        let items = ["libs/maths/src/indexing.rs", "games/game/src/main.rs"];
        let mut matches = Vec::new();
        fuzzy_rank("mairns", &items, &mut matches);
        assert_eq!(matches[0].item, 1);
    }

    #[test]
    fn file_stem_match_beats_scattered_path_match() {
        let items = ["libs/maths/src/indexing.rs", "games/game/src/main.rs"];
        let mut matches = Vec::new();
        fuzzy_rank("mainrs", &items, &mut matches);
        assert_eq!(matches[0].item, 1);
    }

    #[test]
    fn compact_file_stem_beats_longer_filename_match() {
        let items = [
            "libs/asset_manager/src/maybe_new.rs",
            "games/game/src/main.rs",
        ];
        let mut matches = Vec::new();
        fuzzy_rank("marns", &items, &mut matches);
        assert_eq!(matches[0].item, 1, "matches: {matches:?}");
    }

    #[test]
    fn parallel_scoring_keeps_item_indices_and_ranking() {
        let mut owned_items = Vec::with_capacity(FUZZY_PARALLEL_ITEMS + 1);
        for item in 0..FUZZY_PARALLEL_ITEMS {
            owned_items.push(format!("generated/path/{item}/unrelated.txt"));
        }
        owned_items.push("games/game/src/main.rs".to_string());
        let mut item_refs = Vec::with_capacity(owned_items.len());
        item_refs.extend(owned_items.iter().map(String::as_str));
        let mut matches = Vec::new();
        fuzzy_rank("mainrs", &item_refs, &mut matches);
        assert_eq!(matches[0].item, FUZZY_PARALLEL_ITEMS);
    }
}
