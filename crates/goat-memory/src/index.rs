//! In-memory episodic recall index.
//!
//! Eliminates the per-turn full-table decode that `search_episodic` previously
//! performed. Instead, each persona's embeddings are kept in a **flat
//! `Vec<f32>` arena** (`n × dim` contiguous buffer) in RAM so every turn's
//! KNN is a single in-process pass with no SQL round-trip.
//!
//! # Design
//!
//! * **Permanent full-history recall.** No rows are deleted or pruned.
//!   The arena grows monotonically, and every entry is always a candidate.
//! * **Lazy per-persona hydration.** The partition for a persona is loaded from
//!   the DB on first `insert`/`search` call, not at construction time. This
//!   bounds startup cost to active personas and keeps `SqliteMemory::from_pool`
//!   sync.
//! * **`text` stays on disk.** Only embeddings + id/kind/ts metadata live in
//!   RAM, keeping resident bytes proportional to `n × dim × 4` rather than
//!   `n × (dim × 4 + avg_text_len)`.
//! * **Concurrency.** Personas process events serially so there is never
//!   write–write contention within one persona. Different personas use
//!   independent partitions (no cross-persona contention). The outer map is
//!   guarded by a `std::sync::Mutex` that is **never held across `.await`**;
//!   the inner partition uses a `tokio::sync::RwLock` (held across `.await`
//!   only during one-shot hydration).
//!
//! # Escalation
//!
//! The `EpisodicIndex` trait allows future backends to be swapped in without
//! touching `goat-brain` or the caller surface:
//! * N > ~1 M rows / persona → approximate HNSW with an exact top-K fallback.
//! * Hard RAM budget → sqlite-vec extension (requires build change).
//! * Boot latency → f16 / int8 arena quantisation.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use goat_types::PersonaId;
use sqlx::sqlite::SqlitePool;
use tracing::{info, warn};

use crate::{decode_vec, EpisodicKind};

// ── Public surface ─────────────────────────────────────────────────────────

/// A single result from an episodic index search.
#[derive(Clone, Debug)]
pub struct EpisodicHit {
    /// Row id in `episodic_memory`.
    pub id: String,
    pub kind: EpisodicKind,
    pub ts: DateTime<Utc>,
    /// Cosine similarity to the query vector (0..=1).
    pub score: f32,
}

/// Abstraction over episodic memory KNN backends.
///
/// The only production implementation is [`InMemoryEpisodicIndex`]; the trait
/// exists to allow future backends to be swapped without touching `goat-brain`
/// or the `MemoryStore` public surface.
///
/// Implementations must be `Send + Sync + 'static` and must never hold a lock
/// (or perform I/O) in a way that blocks the Tokio runtime indefinitely.
#[async_trait]
pub trait EpisodicIndex: Send + Sync + 'static {
    /// Adds a single entry to the index. Called immediately after a successful
    /// `INSERT INTO episodic_memory` so the index and DB stay consistent.
    /// No-op if `embedding` is empty.
    async fn insert(
        &self,
        persona: PersonaId,
        id: &str,
        ts: DateTime<Utc>,
        kind: EpisodicKind,
        embedding: &[f32],
    );

    /// Returns the top-`k` entries by cosine similarity to `query`, ordered
    /// descending. May return fewer than `k` items if the index has fewer
    /// entries. Returns an empty vec if the index has no entries for the given
    /// persona.
    async fn search(&self, persona: PersonaId, query: &[f32], k: usize) -> Vec<EpisodicHit>;
}

// ── InMemoryEpisodicIndex ──────────────────────────────────────────────────

/// Metadata for a single indexed entry (no text — text stays in DB).
struct EntryMeta {
    id: String,
    kind: EpisodicKind,
    ts: DateTime<Utc>,
}

/// Per-persona shard of the index.
struct Partition {
    /// Embedding dimension. Set on first `insert`; subsequent inserts must
    /// match or be skipped with a warning.
    dim: usize,
    /// Flat arena: row `i` occupies `arena[i*dim .. (i+1)*dim]`.
    arena: Vec<f32>,
    /// Parallel metadata for each row.
    meta: Vec<EntryMeta>,
    /// Set of ids already in the arena. Used to prevent duplicates when
    /// `insert` is called before the partition is hydrated from the DB: the
    /// first `insert` triggers hydration, which loads the just-inserted row
    /// from the DB; the id set ensures the subsequent `push` skips it.
    ids: std::collections::HashSet<String>,
    /// Whether the partition has been hydrated from the DB.
    loaded: bool,
}

impl Partition {
    fn new() -> Self {
        Self {
            dim: 0,
            arena: Vec::new(),
            meta: Vec::new(),
            ids: std::collections::HashSet::new(),
            loaded: false,
        }
    }

    /// Appends one entry. Silently skips if the id is already present
    /// (dedup guard) or if the embedding dimension does not match.
    fn push(&mut self, id: String, kind: EpisodicKind, ts: DateTime<Utc>, embedding: &[f32]) {
        if embedding.is_empty() {
            return;
        }
        if self.ids.contains(&id) {
            return; // dedup: already inserted (e.g. loaded by hydration)
        }
        if self.dim == 0 {
            self.dim = embedding.len();
        } else if embedding.len() != self.dim {
            warn!(
                expected_dim = self.dim,
                actual_dim = embedding.len(),
                "episodic index: skipping entry with mismatched dimension"
            );
            return;
        }
        self.arena.extend_from_slice(embedding);
        self.ids.insert(id.clone());
        self.meta.push(EntryMeta { id, kind, ts });
    }

    /// Brute-force cosine KNN over the full arena.
    fn search(&self, query: &[f32], k: usize) -> Vec<EpisodicHit> {
        if k == 0 || self.dim == 0 || query.len() != self.dim || self.meta.is_empty() {
            return Vec::new();
        }
        // Score every entry.
        let mut scored: Vec<(f32, usize)> = self
            .meta
            .iter()
            .enumerate()
            .map(|(i, _)| {
                let row = &self.arena[i * self.dim..(i + 1) * self.dim];
                (crate::cosine(query, row), i)
            })
            .collect();

        // Partial sort: bring the top-k to the front without a full sort.
        if k < scored.len() {
            scored.select_nth_unstable_by(k - 1, |a, b| {
                b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal)
            });
            scored.truncate(k);
        }
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        scored
            .into_iter()
            .map(|(score, i)| {
                let m = &self.meta[i];
                EpisodicHit {
                    id: m.id.clone(),
                    kind: m.kind,
                    ts: m.ts,
                    score,
                }
            })
            .collect()
    }
}

/// An episodic index that holds all embeddings in RAM, partitioned per persona.
///
/// Suitable for single-user deployments with up to ~10k–100k entries per
/// persona (exact upper bound depends on embedding dimension and available
/// RAM). See module-level doc for escalation paths.
pub struct InMemoryEpisodicIndex {
    pool: Arc<SqlitePool>,
    // Outer map: never held across .await. Clone the Arc, then release lock.
    partitions: Mutex<HashMap<PersonaId, Arc<tokio::sync::RwLock<Partition>>>>,
}

impl InMemoryEpisodicIndex {
    pub fn new(pool: Arc<SqlitePool>) -> Self {
        Self {
            pool,
            partitions: Mutex::new(HashMap::new()),
        }
    }

    /// Returns (or creates) the partition Arc for a persona, releasing the
    /// outer mutex immediately.
    fn partition(&self, persona: PersonaId) -> Arc<tokio::sync::RwLock<Partition>> {
        let mut map = self.partitions.lock().expect("episodic index lock");
        map.entry(persona)
            .or_insert_with(|| Arc::new(tokio::sync::RwLock::new(Partition::new())))
            .clone()
    }

    /// Hydrates `partition` from the DB if not yet loaded.
    /// Caller must already hold the write lock.
    ///
    /// Uses `Partition::push` for each row so that the `ids` HashSet is
    /// updated correctly and the dedup guard prevents duplicates in the case
    /// where `insert` was called before hydration.
    async fn hydrate_if_needed(&self, persona: PersonaId, partition: &mut Partition) {
        if partition.loaded {
            return;
        }
        match self.load_from_db(persona, partition).await {
            Ok(n) => {
                let dim = partition.dim;
                info!(
                    persona = %persona,
                    rows = n,
                    dim,
                    "episodic index: hydrated partition from DB",
                );
            }
            Err(e) => {
                warn!(
                    persona = %persona,
                    error = ?e,
                    "episodic index: hydration failed; index will be empty for this session",
                );
            }
        }
        // Mark loaded regardless of success so we don't retry on every search.
        partition.loaded = true;
    }

    /// Streams all episodic rows for `persona` from the DB and pushes them
    /// into `partition` via `Partition::push` (which handles dedup and
    /// dimension validation). Returns the number of rows loaded.
    async fn load_from_db(
        &self,
        persona: PersonaId,
        partition: &mut Partition,
    ) -> Result<usize, sqlx::Error> {
        use futures::StreamExt;
        use sqlx::Row;

        let mut rows = sqlx::query(
            r#"SELECT id, kind, ts, embedding
               FROM episodic_memory
               WHERE persona_id = ? AND embedding IS NOT NULL
               ORDER BY ts"#,
        )
        .bind(persona.to_string())
        .fetch(&*self.pool);

        let mut count = 0usize;
        while let Some(row) = rows.next().await {
            let row = row?;
            let id: String = row.get(0);
            let kind_str: String = row.get(1);
            let ts_str: String = row.get(2);
            let blob: Vec<u8> = row.get(3);

            let embedding = decode_vec(&blob);
            if embedding.is_empty() {
                continue;
            }
            let ts = crate::parse_ts_pub(&ts_str);
            // push handles dedup (ids HashSet) and dimension checks.
            partition.push(id, EpisodicKind::parse(&kind_str), ts, &embedding);
            count += 1;
        }

        Ok(count)
    }
}

#[async_trait]
impl EpisodicIndex for InMemoryEpisodicIndex {
    async fn insert(
        &self,
        persona: PersonaId,
        id: &str,
        ts: DateTime<Utc>,
        kind: EpisodicKind,
        embedding: &[f32],
    ) {
        if embedding.is_empty() {
            return;
        }
        let part_arc = self.partition(persona);
        let mut guard = part_arc.write().await;
        self.hydrate_if_needed(persona, &mut guard).await;
        guard.push(id.to_string(), kind, ts, embedding);
    }

    async fn search(&self, persona: PersonaId, query: &[f32], k: usize) -> Vec<EpisodicHit> {
        if query.is_empty() || k == 0 {
            return Vec::new();
        }
        let part_arc = self.partition(persona);

        // Fast path: if already loaded, use read lock (no await for search itself).
        {
            let guard = part_arc.read().await;
            if guard.loaded {
                return guard.search(query, k);
            }
        }

        // Slow path: hydrate under write lock, then search.
        {
            let mut guard = part_arc.write().await;
            self.hydrate_if_needed(persona, &mut guard).await;
            guard.search(query, k)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn mk_partition(dim: usize, vecs: &[(&str, Vec<f32>)]) -> Partition {
        let mut p = Partition::new();
        for (id, v) in vecs {
            p.push(id.to_string(), EpisodicKind::User, Utc::now(), v);
        }
        assert_eq!(p.dim, dim);
        p
    }

    #[test]
    fn search_returns_top_k_by_cosine() {
        let p = mk_partition(
            2,
            &[
                ("cats", vec![1.0, 0.0]),
                ("dogs", vec![0.0, 1.0]),
                ("fish", vec![0.7, 0.7]),
            ],
        );
        let hits = p.search(&[1.0, 0.0], 1);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "cats");
        assert!((hits[0].score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn search_all_history_preserved() {
        // All 4 entries reachable even though only k=1 is requested per query
        let p = mk_partition(
            2,
            &[
                ("a", vec![1.0, 0.0]),
                ("b", vec![0.0, 1.0]),
                ("c", vec![-1.0, 0.0]),
                ("d", vec![0.0, -1.0]),
            ],
        );
        // Query for each axis: the correct entry must win
        let hit_a = p.search(&[1.0, 0.0], 1);
        let hit_b = p.search(&[0.0, 1.0], 1);
        let hit_c = p.search(&[-1.0, 0.0], 1);
        let hit_d = p.search(&[0.0, -1.0], 1);
        assert_eq!(hit_a[0].id, "a");
        assert_eq!(hit_b[0].id, "b");
        assert_eq!(hit_c[0].id, "c");
        assert_eq!(hit_d[0].id, "d");
    }

    #[test]
    fn search_empty_partition_returns_empty() {
        let p = Partition::new();
        let hits = p.search(&[1.0, 0.0], 5);
        assert!(hits.is_empty());
    }

    #[test]
    fn search_respects_k_bound() {
        let p = mk_partition(
            2,
            &[
                ("a", vec![1.0, 0.0]),
                ("b", vec![0.9, 0.1]),
                ("c", vec![0.8, 0.2]),
            ],
        );
        let hits = p.search(&[1.0, 0.0], 2);
        assert_eq!(hits.len(), 2);
    }
}
