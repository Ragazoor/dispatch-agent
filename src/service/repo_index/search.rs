//! RAG query side: scanning stored chunks and ranking them against a query
//! embedding by cosine similarity.

use std::path::Path;

use anyhow::Result;

use crate::service::embeddings::{
    cosine_similarity, deserialize_embedding, RAG_SIMILARITY_THRESHOLD,
};

use super::scan::open_rag_db;
use super::SearchResult;

/// Score `rows` against `query_vec`, drop anything below `threshold`, sort by
/// descending score, and truncate to `limit`.
///
/// Pure and DB-free: each row is `(file_path, chunk_text, embedding_bytes)`.
pub(crate) fn score_and_rank(
    query_vec: &[f32],
    rows: impl Iterator<Item = (String, String, Vec<u8>)>,
    threshold: f32,
    limit: usize,
) -> Vec<SearchResult> {
    let mut results: Vec<SearchResult> = rows
        .filter_map(|(path, text, blob)| {
            let emb = deserialize_embedding(&blob);
            let score = cosine_similarity(query_vec, &emb);
            if score < threshold {
                return None;
            }
            Some(SearchResult {
                file_path: path,
                chunk_text: text,
                score,
            })
        })
        .collect();
    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(limit);
    results
}

/// Read every chunk row from the per-repo store and rank them against
/// `query_vec`. Runs entirely on the calling (blocking) thread.
///
/// The scan is unbounded by design: no pre-scoring row limit is meaningful,
/// because there is no order to sort by until similarity has been computed.
/// Cost is therefore linear in the number of stored chunks.
pub(crate) fn query_and_rank(
    repo_path: &Path,
    query_vec: &[f32],
    limit: usize,
) -> Result<Vec<SearchResult>> {
    let conn = open_rag_db(repo_path)?;
    let mut stmt = conn.prepare("SELECT file_path, chunk_text, embedding FROM rag_chunks")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Vec<u8>>(2)?,
        ))
    })?;
    // Rows that fail to decode are skipped (preserves prior `row.ok()?` behaviour).
    let collected = rows.filter_map(|row| row.ok());
    Ok(score_and_rank(
        query_vec,
        collected,
        RAG_SIMILARITY_THRESHOLD,
        limit,
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::service::embeddings::serialize_embedding;

    fn row(path: &str, text: &str, emb: &[f32]) -> (String, String, Vec<u8>) {
        (path.to_string(), text.to_string(), serialize_embedding(emb))
    }

    #[test]
    fn score_and_rank_filters_below_threshold() {
        let query = [1.0f32, 0.0];
        let rows = vec![
            row("hit.md", "relevant", &[1.0, 0.0]),    // cosine 1.0
            row("miss.md", "orthogonal", &[0.0, 1.0]), // cosine 0.0 < 0.25
        ];
        let results = score_and_rank(&query, rows.into_iter(), 0.25, 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].file_path, "hit.md");
    }

    #[test]
    fn score_and_rank_sorts_descending_by_score() {
        let query = [1.0f32, 0.0];
        let rows = vec![
            row("mid.md", "partial", &[1.0, 1.0]), // cosine ~0.707
            row("top.md", "exact", &[1.0, 0.0]),   // cosine 1.0
        ];
        let results = score_and_rank(&query, rows.into_iter(), 0.25, 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].file_path, "top.md");
        assert_eq!(results[1].file_path, "mid.md");
        assert!(results[0].score >= results[1].score);
    }

    #[test]
    fn score_and_rank_truncates_to_limit() {
        let query = [1.0f32, 0.0];
        let rows = vec![
            row("a.md", "a", &[1.0, 0.0]),
            row("b.md", "b", &[1.0, 0.0]),
            row("c.md", "c", &[1.0, 0.0]),
        ];
        let results = score_and_rank(&query, rows.into_iter(), 0.25, 2);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn score_and_rank_empty_rows_returns_empty() {
        let query = [1.0f32, 0.0];
        let results = score_and_rank(&query, std::iter::empty(), 0.25, 5);
        assert!(results.is_empty());
    }

    #[test]
    fn score_and_rank_all_below_threshold_returns_empty() {
        let query = [1.0f32, 0.0];
        let rows = vec![row("miss.md", "orthogonal", &[0.0, 1.0])];
        let results = score_and_rank(&query, rows.into_iter(), 0.25, 5);
        assert!(results.is_empty());
    }

    #[test]
    fn score_and_rank_threshold_boundary_is_inclusive() {
        // A score exactly at the threshold is kept (filter drops `score < threshold`).
        let query = [1.0f32, 0.0];
        let rows = vec![row("edge.md", "edge", &[1.0, 0.0])]; // cosine 1.0
        let results = score_and_rank(&query, rows.into_iter(), 1.0, 5);
        assert_eq!(results.len(), 1);
    }
}
