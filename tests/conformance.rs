//! Every backend Freyja ships, and one written outside it, against the
//! published conformance check.

use freyja::testing::storage_conformance;
use freyja::{InMemoryStorage, JsonlStorage, Message, Storage, StorageFuture};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The simplest backend a crate outside Freyja could write.
#[derive(Default)]
struct Outside(Vec<Message>);

impl Storage for Outside {
    fn load(&mut self) -> StorageFuture<'_, Vec<Message>> {
        Box::pin(async move { Ok(self.0.clone()) })
    }
    fn append(&mut self, messages: Vec<Message>) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            self.0.extend(messages);
            Ok(())
        })
    }
    fn clear(&mut self) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            self.0.clear();
            Ok(())
        })
    }
}

/// Hands messages back newest first, which the check must catch.
#[derive(Default)]
struct Reversed(Vec<Message>);

impl Storage for Reversed {
    fn load(&mut self) -> StorageFuture<'_, Vec<Message>> {
        Box::pin(async move { Ok(self.0.iter().rev().cloned().collect()) })
    }
    fn append(&mut self, messages: Vec<Message>) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            self.0.extend(messages);
            Ok(())
        })
    }
    fn clear(&mut self) -> StorageFuture<'_, ()> {
        Box::pin(async move {
            self.0.clear();
            Ok(())
        })
    }
}

#[tokio::test]
async fn in_memory_storage_conforms() {
    storage_conformance(InMemoryStorage::new).await;
}

#[tokio::test]
async fn a_plain_vector_conforms() {
    storage_conformance(Vec::<Message>::new).await;
}

#[tokio::test]
async fn a_backend_written_outside_freyja_conforms() {
    storage_conformance(Outside::default).await;
}

#[tokio::test]
async fn jsonl_storage_conforms() {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let make = || {
        let path = std::env::temp_dir().join(format!(
            "freyja-conformance-{}-{}.jsonl",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_file(&path);
        JsonlStorage::open(path).expect("open")
    };
    storage_conformance(make).await;
}

#[tokio::test]
#[should_panic(expected = "oldest first")]
async fn a_backend_that_reverses_order_fails() {
    storage_conformance(Reversed::default).await;
}
