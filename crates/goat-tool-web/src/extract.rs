use std::collections::{BTreeSet, HashMap, HashSet};

const K1: f64 = 1.5;
const B: f64 = 0.75;

const STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "of", "to", "in", "on", "for", "with", "as", "at", "by", "be",
    "is", "are", "was", "were", "this", "that", "it", "its", "from", "into", "how", "what", "why",
    "do", "does",
];

pub(crate) fn extract_relevant(markdown: &str, query: &str, max_passages: usize) -> Option<String> {
    let query_terms = tokenize(query);
    if query_terms.is_empty() {
        return None;
    }
    let blocks: Vec<&str> = split_passages(markdown);
    if blocks.is_empty() {
        return None;
    }

    let token_lists: Vec<Vec<String>> = blocks.iter().map(|block| tokenize(block)).collect();
    let count = blocks.len();
    let total_len: usize = token_lists.iter().map(Vec::len).sum();
    let avgdl = as_f64(total_len) / as_f64(count.max(1));

    let mut df: HashMap<&str, usize> = HashMap::new();
    for tokens in &token_lists {
        let unique: HashSet<&str> = tokens.iter().map(String::as_str).collect();
        for term in unique {
            *df.entry(term).or_default() += 1;
        }
    }

    let query_unique: HashSet<&str> = query_terms.iter().map(String::as_str).collect();
    let scores: Vec<f64> = token_lists
        .iter()
        .map(|tokens| bm25(tokens, &query_unique, &df, count, avgdl))
        .collect();

    let is_heading: Vec<bool> = blocks
        .iter()
        .map(|block| block.trim_start().starts_with('#'))
        .collect();

    let mut ranked: Vec<usize> = (0..count).filter(|&i| scores[i] > 0.0).collect();
    if ranked.is_empty() {
        return None;
    }
    ranked.sort_by(|&a, &b| {
        scores[b]
            .partial_cmp(&scores[a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    ranked.truncate(max_passages);

    let mut selected: BTreeSet<usize> = ranked.iter().copied().collect();
    for &i in &ranked {
        if !is_heading[i]
            && let Some(heading) = (0..i).rev().find(|&j| is_heading[j])
        {
            selected.insert(heading);
        }
    }

    let mut out = String::new();
    let mut prev: Option<usize> = None;
    for &i in &selected {
        if let Some(previous) = prev {
            if i > previous + 1 {
                out.push_str("\n\n[\u{2026}]\n\n");
            } else {
                out.push_str("\n\n");
            }
        }
        out.push_str(blocks[i].trim());
        prev = Some(i);
    }
    Some(out)
}

fn bm25(
    tokens: &[String],
    query: &HashSet<&str>,
    df: &HashMap<&str, usize>,
    n: usize,
    avgdl: f64,
) -> f64 {
    let dl = as_f64(tokens.len());
    let mut score = 0.0;
    for &term in query {
        let freq = tokens.iter().filter(|token| token.as_str() == term).count();
        if freq == 0 {
            continue;
        }
        let df_term = df.get(term).copied().unwrap_or(0);
        let idf = (((as_f64(n) - as_f64(df_term) + 0.5) / (as_f64(df_term) + 0.5)) + 1.0).ln();
        let tf = as_f64(freq);
        score += idf * (tf * (K1 + 1.0)) / (tf + K1 * (1.0 - B + B * dl / avgdl));
    }
    score
}

fn split_passages(markdown: &str) -> Vec<&str> {
    markdown
        .split("\n\n")
        .map(str::trim)
        .filter(|block| !block.is_empty())
        .collect()
}

fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|token| token.len() >= 2)
        .map(str::to_lowercase)
        .filter(|token| !STOPWORDS.contains(&token.as_str()))
        .collect()
}

#[allow(clippy::cast_precision_loss)]
fn as_f64(n: usize) -> f64 {
    n as f64
}

#[cfg(test)]
mod tests {
    use super::extract_relevant;

    const DOC: &str = "# Installation\n\nRun the installer script to set up the tool on macOS and Linux.\n\n# Configuration\n\nThe config file lives in your home directory and controls themes and providers.\n\n# Rate limits\n\nThe search provider enforces a rate limit of one request per second on the free tier.";

    #[test]
    fn returns_passage_matching_query() {
        let out = extract_relevant(DOC, "rate limit request per second", 3).expect("match");
        assert!(out.contains("rate limit of one request per second"));
        assert!(out.contains("# Rate limits"));
        assert!(!out.contains("installer script"));
    }

    #[test]
    fn includes_gap_marker_between_disjoint_hits() {
        let out = extract_relevant(DOC, "installer rate limit", 4).expect("match");
        assert!(out.contains('\u{2026}'));
    }

    #[test]
    fn no_match_returns_none() {
        assert!(extract_relevant(DOC, "kubernetes helm chart", 3).is_none());
    }

    #[test]
    fn empty_query_returns_none() {
        assert!(extract_relevant(DOC, "the a of", 3).is_none());
    }
}
