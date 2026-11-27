//! Checks a crate outside Freyja runs against its own implementations.

use crate::{Message, Role, Storage};
use serde_json::json;

/// Checks that a [`Storage`] honours the rules the trait documents.
///
/// `make` returns a new, empty backend each time it is called. Run it from
/// the backend crate's own tests, under any async runtime:
///
/// ```
/// # async fn run() {
/// freyja::testing::storage_conformance(freyja::InMemoryStorage::new).await;
/// # }
/// ```
///
/// # Panics
///
/// On the first broken rule, with a message naming it.
pub async fn storage_conformance<S: Storage>(mut make: impl FnMut() -> S) {
    let first = || {
        vec![
            Message::text(Role::User, "one"),
            Message::text(Role::Assistant, "two"),
        ]
    };
    let second = || vec![Message::text(Role::User, "three")];

    let mut storage = make();
    assert!(
        storage
            .load()
            .await
            .expect("load a new conversation")
            .is_empty(),
        "a new backend must load an empty conversation"
    );

    storage.append(first()).await.expect("append");
    storage.append(second()).await.expect("append");
    let mut expected = first();
    expected.extend(second());
    assert_eq!(
        storage.load().await.expect("load"),
        expected,
        "load must return every appended message, oldest first"
    );

    storage
        .clear()
        .await
        .expect("clear a conversation that holds messages");
    assert!(
        storage.load().await.expect("load after clear").is_empty(),
        "clear must empty the conversation"
    );
    storage
        .clear()
        .await
        .expect("clear must succeed on an empty conversation");

    let mut storage = make();
    storage
        .set_state("conformance", json!(1))
        .await
        .expect("set state");
    if storage
        .state("conformance")
        .await
        .expect("read state")
        .is_some()
    {
        storage
            .set_state("conformance", json!(2))
            .await
            .expect("set state");
        assert_eq!(
            storage.state("conformance").await.expect("read state"),
            Some(json!(2)),
            "state must return the latest value set"
        );
        assert_eq!(
            storage.state("unset").await.expect("read state"),
            None,
            "a key never set must read as None"
        );
        storage.clear().await.expect("clear");
        assert_eq!(
            storage.state("conformance").await.expect("read state"),
            None,
            "clear must remove state"
        );
    }
}
