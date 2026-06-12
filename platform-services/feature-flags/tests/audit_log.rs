use feature_flags::model::{ValueType, Variant};
use feature_flags::store::Store;
use serde_json::Value;
use sqlx::PgPool;

#[sqlx::test(migrations = "./migrations")]
async fn list_changes_records_and_filters_audit_log(pool: PgPool) {
    let store = Store::new(pool);
    let variants = [Variant {
        key: "on".into(),
        value: Value::Bool(true),
    }];
    store
        .create_flag("alice", "flag-a", ValueType::Boolean, true, "on", &variants)
        .await
        .unwrap();
    store
        .update_flag("bob", "flag-a", false, "on")
        .await
        .unwrap();

    let all = store.list_changes("", "", 0).await.unwrap();
    assert_eq!(all.len(), 2);
    // Newest first: the update precedes the create.
    assert_eq!(all[0].action, "update_flag");
    assert_eq!(all[0].actor, "bob");
    assert_eq!(all[1].action, "create_flag");
    assert_eq!(all[1].actor, "alice");

    let filtered = store.list_changes("flag", "flag-a", 0).await.unwrap();
    assert_eq!(filtered.len(), 2);
    let other = store.list_changes("segment", "nope", 0).await.unwrap();
    assert!(other.is_empty());

    let limited = store.list_changes("", "", 1).await.unwrap();
    assert_eq!(limited.len(), 1);
    assert_eq!(limited[0].action, "update_flag");
}
