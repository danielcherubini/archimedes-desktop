//! The TRUST lookup seam (ADR 0010/0025): `SqliteTrustSource` (the
//! current `db.space_trusted` lookup — the permission gate's use (a))
//! / `StaticTrustSource` (the Worker — the flag arrives in the `start`
//! envelope, updated by `config` messages AND by the `trust-space`
//! permission outcome — Task 2's `WorkerCore`).
//!
//! The fail-closed semantics live in the sources themselves (a lookup
//! that cannot resolve is `false`); the `Option` at the `AgentLoop`
//! call site adds a second layer (`None` = always prompt).

use std::sync::{Arc, Mutex};

use crate::storage::Db;

/// The TRUST lookup seam (ADR 0010/0025): `SqliteTrustSource` (the
/// current `db.space_trusted` lookup — the gate's use (a)) /
/// `StaticTrustSource` (the Worker — the flag arrives in the `start`
/// envelope, updated by `config` messages AND by the `trust-space`
/// permission outcome — Task 2's `WorkerCore`).
pub trait TrustSource: Send + Sync {
    /// Is the Space at `cwd` trusted? Fail-closed: a lookup that cannot
    /// resolve is `false` (untrusted — the gate prompts).
    fn is_trusted(&self, cwd: &std::path::Path) -> bool;
}

/// The SQLite trust source (the current `db.space_trusted` lookup, moved
/// verbatim — a canonicalize failure or a missing row is `false` —
/// untrusted, fail-closed).
pub struct SqliteTrustSource(Arc<Db>);

impl SqliteTrustSource {
    pub fn new(db: Arc<Db>) -> Self {
        Self(db)
    }
}

impl TrustSource for SqliteTrustSource {
    fn is_trusted(&self, cwd: &std::path::Path) -> bool {
        self.0.space_trusted(cwd).unwrap_or(false)
    }
}

/// The static trust source (the Worker — the flag arrives in the `start`
/// envelope, updated by `config` messages AND by the `trust-space`
/// permission outcome): a single `bool` behind a `Mutex` (the `config`
/// `{ trusted }` update + the `trust-space` outcome flip). `Clone`
/// shares the flag (the `Arc`) — a clone observes flips.
#[derive(Clone)]
pub struct StaticTrustSource(Arc<Mutex<bool>>);

impl StaticTrustSource {
    pub fn new(trusted: bool) -> Self {
        Self(Arc::new(Mutex::new(trusted)))
    }

    /// The `config { trusted }` update + the `trust-space` outcome flip.
    pub fn set(&self, trusted: bool) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = trusted;
    }
}

impl TrustSource for StaticTrustSource {
    fn is_trusted(&self, _cwd: &std::path::Path) -> bool {
        *self.0.lock().unwrap_or_else(|p| p.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::{SqliteTrustSource, StaticTrustSource, TrustSource};
    use crate::storage::Db;
    use std::sync::Arc;

    /// A temp-dir `Db` + a REAL temp `cwd` dir (the `spaces` rows are
    /// keyed by the CANONICAL path — a nonexistent path canonicalize-
    /// fails and `space_trusted` fails closed to `false`).
    fn temp_db_cwd() -> (Arc<Db>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("harness-trust-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Arc::new(Db::open(&dir.join("t.db")).expect("db should open"));
        let cwd = dir.join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        (db, cwd)
    }

    /// `StaticTrustSource` — `new(false)` is untrusted; `set(true)`
    /// flips it (the `config { trusted }` update + the `trust-space`
    /// outcome flip); `new(true)` is trusted from the start (the
    /// `start` envelope's `trusted` flag).
    #[test]
    fn static_trust_source_new_and_set_flip_the_flag() {
        let s = StaticTrustSource::new(false);
        assert!(!s.is_trusted(std::path::Path::new("/whatever")));
        s.set(true);
        assert!(
            s.is_trusted(std::path::Path::new("/whatever")),
            "set(true) flips it"
        );
        s.set(false);
        assert!(
            !s.is_trusted(std::path::Path::new("/whatever")),
            "set(false) flips it back"
        );

        let s = StaticTrustSource::new(true);
        assert!(s.is_trusted(std::path::Path::new("/whatever")));
    }

    /// `StaticTrustSource` is `Send + Sync` behind an `Arc` (the
    /// `AgentLoop` holds `Option<Arc<dyn TrustSource>>` — the trait
    /// bound requires it), and the flip is visible through the trait
    /// object.
    #[test]
    fn static_trust_source_is_the_trust_source_seam() {
        let s = Arc::new(StaticTrustSource::new(false));
        let s_dyn: Arc<dyn TrustSource> = s.clone();
        assert!(!s_dyn.is_trusted(std::path::Path::new("/x")));
        s.set(true);
        assert!(s_dyn.is_trusted(std::path::Path::new("/x")));
    }

    /// `SqliteTrustSource` — a temp-dir `Db` with a trusted `spaces`
    /// row returns `true` (the exact `db.space_trusted` behavior, moved
    /// verbatim); an untrusted / missing row is `false` (fail-closed);
    /// a nonexistent `cwd` canonicalize-fails → `false`.
    #[test]
    fn sqlite_trust_source_reads_the_space_flag() {
        let (db, cwd) = temp_db_cwd();
        let source = SqliteTrustSource::new(db.clone());
        // No `spaces` row yet → fail-closed `false`.
        assert!(!source.is_trusted(&cwd));
        db.upsert_space(&cwd.display().to_string(), false).unwrap();
        assert!(!source.is_trusted(&cwd), "an untrusted row is `false`");
        db.set_space_trusted(&cwd.display().to_string(), true)
            .unwrap();
        assert!(source.is_trusted(&cwd), "a trusted row is `true`");
        // A nonexistent path canonicalize-fails → `false` (fail-closed).
        assert!(!source.is_trusted(std::path::Path::new("/definitely/missing/path")));
    }
}
