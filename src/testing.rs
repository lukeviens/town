//! Waiting for a word, in tests: never a window of time, only a guard, so a test that would hang
//! fails. Shared by the tests in town and the examples' test beside it.

use std::time::Duration;

use tokio::sync::broadcast::{error::RecvError, Receiver};

/// what `f` comes to, guarded
pub async fn guard<T>(f: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(30), f).await.expect("never came")
}

/// the first word that `is`, as the town says it
pub async fn until<W: Clone>(words: &mut Receiver<W>, is: impl Fn(&W) -> bool) -> W {
    guard(async {
        loop {
            match words.recv().await {
                Ok(w) if is(&w) => return w,
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => panic!("the town closed"),
            }
        }
    })
    .await
}

/// every word the town says, up to and including the first that `is`
pub async fn upto<W: Clone>(words: &mut Receiver<W>, is: impl Fn(&W) -> bool) -> Vec<W> {
    let mut said: Vec<W> = Vec::new();
    while said.last().is_none_or(|w| !is(w)) {
        said.push(until(words, |_| true).await);
    }
    said
}
