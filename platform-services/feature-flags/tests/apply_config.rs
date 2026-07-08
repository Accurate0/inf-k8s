use feature_flags::model::{Flag, Segment, ValueType, Variant};
use feature_flags::store::{ChangeOp, Store};
use serde_json::json;
use sqlx::PgPool;

fn bool_flag(key: &str, enabled: bool) -> Flag {
    Flag {
        key: key.into(),
        value_type: ValueType::Boolean,
        enabled,
        default_variant_key: "off".into(),
        archived: false,
        variants: vec![
            Variant {
                key: "on".into(),
                value: json!(true),
            },
            Variant {
                key: "off".into(),
                value: json!(false),
            },
        ],
        rules: vec![],
    }
}

fn segment(key: &str) -> Segment {
    Segment {
        key: key.into(),
        name: "Beta".into(),
        constraints: vec![],
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn dry_run_reports_diff_without_writing(pool: PgPool) {
    let store = Store::new(pool);
    let before = store.config_version().await.unwrap();

    let out = store
        .apply_config("alice", &[bool_flag("f", true)], &[], true, 0)
        .await
        .unwrap();

    assert!(!out.applied);
    assert_eq!(out.changes.len(), 1);
    assert_eq!(out.changes[0].op, ChangeOp::Create);
    assert_eq!(out.from_version, before);
    assert_eq!(out.to_version, before);
    // Nothing was written.
    assert_eq!(store.config_version().await.unwrap(), before);
    assert!(store.load_snapshot().await.unwrap().flags.is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn apply_creates_then_is_idempotent(pool: PgPool) {
    let store = Store::new(pool);
    let flags = [bool_flag("f", true)];
    let segments = [segment("beta")];

    let out = store
        .apply_config("alice", &flags, &segments, false, 0)
        .await
        .unwrap();
    assert!(out.applied);
    assert_eq!(out.changes.len(), 2);
    assert!(out.to_version > out.from_version);

    let snap = store.load_snapshot().await.unwrap();
    assert!(snap.flags.contains_key("f"));
    assert!(snap.segments.contains_key("beta"));

    // Re-applying identical desired state is a no-op: no changes, no version bump.
    let again = store
        .apply_config("alice", &flags, &segments, false, 0)
        .await
        .unwrap();
    assert!(again.applied);
    assert!(again.changes.is_empty());
    assert_eq!(again.from_version, again.to_version);
}

#[sqlx::test(migrations = "./migrations")]
async fn update_is_detected(pool: PgPool) {
    let store = Store::new(pool);
    store
        .apply_config("alice", &[bool_flag("f", true)], &[], false, 0)
        .await
        .unwrap();

    let out = store
        .apply_config("alice", &[bool_flag("f", false)], &[], true, 0)
        .await
        .unwrap();
    assert_eq!(out.changes.len(), 1);
    assert_eq!(out.changes[0].op, ChangeOp::Update);
    assert_eq!(out.changes[0].detail, json!({ "fields": ["enabled"] }));
}

#[sqlx::test(migrations = "./migrations")]
async fn absent_flags_and_segments_are_pruned(pool: PgPool) {
    let store = Store::new(pool);
    store
        .apply_config("alice", &[bool_flag("f", true)], &[segment("beta")], false, 0)
        .await
        .unwrap();

    let out = store.apply_config("alice", &[], &[], false, 0).await.unwrap();
    assert!(out.applied);
    let ops: Vec<_> = out.changes.iter().map(|c| c.op).collect();
    assert_eq!(ops, vec![ChangeOp::Delete, ChangeOp::Delete]);

    let snap = store.load_snapshot().await.unwrap();
    assert!(snap.flags.is_empty());
    assert!(snap.segments.is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn stale_expected_version_aborts(pool: PgPool) {
    let store = Store::new(pool);
    store
        .apply_config("alice", &[bool_flag("f", true)], &[], false, 0)
        .await
        .unwrap();
    let current = store.config_version().await.unwrap();

    // A pruning apply against a stale version must abort rather than delete.
    let err = store
        .apply_config("alice", &[], &[], false, current + 999)
        .await
        .unwrap_err();
    assert!(matches!(err, feature_flags::error::AppError::Aborted(_)));
    // The flag is untouched.
    assert!(store.load_snapshot().await.unwrap().flags.contains_key("f"));
}
