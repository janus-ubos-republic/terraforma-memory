//! Derived, bounded lexical retrieval. Ranking follows the existing Desk Drive
//! (drive.rs): +3 term overlap, +2 title overlap, +8 literal phrase, stable ties.
//! This adapter preserves single-character identifiers and groups identical text.
use crate::{sha256, Result, World};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

pub fn tokenize(value: &str) -> BTreeSet<String> {
    value
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
        .collect()
}

pub fn query(world: &World, input: &str, limit: usize) -> Result<Value> {
    let started = Instant::now();
    let input = input.trim();
    let terms = tokenize(input);
    if input.len() > 512 || terms.is_empty() || terms.len() > 32 || !(1..=20).contains(&limit) {
        return Err(
            "search needs 1..32 terms, at most 512 bytes and a result limit of 1..20".into(),
        );
    }
    let mut hits = Vec::new();
    let mut duplicates: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let records = world.records();
    for r in records.values() {
        duplicates
            .entry(r.source_sha256.clone())
            .or_default()
            .push(r.id.clone());
        let title_terms = tokenize(&r.title);
        let mut all = tokenize(&r.text);
        all.extend(title_terms.iter().cloned());
        let matched: Vec<_> = terms.intersection(&all).cloned().collect();
        if matched.is_empty() {
            continue;
        }
        let missing: Vec<_> = terms.difference(&all).cloned().collect();
        let score = matched.len() * 3
            + terms.intersection(&title_terms).count() * 2
            + if r.text.to_lowercase().contains(&input.to_lowercase()) {
                8
            } else {
                0
            };
        let (line_number, line) = r
            .text
            .lines()
            .enumerate()
            .find(|(_, line)| !tokenize(line).is_disjoint(&terms))
            .unwrap_or((0, ""));
        hits.push(json!({"id":r.id,"title":r.title,"score":score,
            "status":if missing.is_empty(){"full_match"}else{"partial_match"},
            "matched_terms":matched,"missing_terms":missing,
            "excerpt":line.chars().take(320).collect::<String>(),"line":line_number+1,
            "excerpt_truncated":line.chars().count()>320,
            "source_sha256":r.source_sha256,"ring":r.ring}));
    }
    // Full coverage precedes score, so a high-scoring partial hit cannot hide a full match.
    hits.sort_by(|a, b| {
        let full = |v: &Value| v["status"] == "full_match";
        full(b)
            .cmp(&full(a))
            .then_with(|| b["score"].as_u64().cmp(&a["score"].as_u64()))
            .then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
    });
    let mut seen = BTreeSet::new();
    hits.retain(|h| seen.insert(h["source_sha256"].as_str().unwrap().to_string()));
    let total = hits.len();
    hits.truncate(limit);
    for h in &mut hits {
        h["source_ids_with_identical_text"] =
            json!(duplicates[h["source_sha256"].as_str().unwrap()]);
    }
    let status = if hits.is_empty() {
        "no_match"
    } else if hits.iter().any(|h| h["status"] == "full_match") {
        "full_match"
    } else {
        "partial_match"
    };
    let body = json!({"schema":"ubos.terraforma-local-search.v1", "engine":"desk-lexical-adapter-v1",
        "query":input,"terms":terms,"status":status,"estate_id":world.estate_id,
        "world_sha256":world.digest()?,"journal_head_sha256":world.journal_head,
        "record_count":records.len(),"unique_match_count":total,"result_limit":limit,
        "truncated":total>limit,"hits":hits,
        "coverage":"Saved UTF-8 text and titles; exact case-insensitive words; no semantic answers or external files.",
        "egress":"none"});
    let digest = sha256(&serde_json::to_vec(&body).map_err(|e| e.to_string())?);
    // Timing is observation, excluded from the reproducible result digest.
    Ok(json!({"result":body,"result_sha256":digest,"elapsed_micros":started.elapsed().as_micros()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_identifiers_and_unicode() {
        assert_eq!(
            tokenize("7 42 142 Málaga MÁLAGA a A x_y"),
            BTreeSet::from(["7", "42", "142", "málaga", "a", "x_y"].map(str::to_string))
        );
    }
}
