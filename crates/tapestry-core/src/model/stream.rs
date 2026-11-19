use std::collections::VecDeque;

use async_trait::async_trait;
use tapestry_protocol::{ModelResponse, ModelStreamItem};

use crate::error::CoreError;

/// Pull-based source behind a [`ModelStream`], for clients that produce items
/// incrementally (for example from a provider stream).
#[async_trait]
pub trait ModelStreamSource: Send {
    async fn next(&mut self) -> Option<Result<ModelStreamItem, CoreError>>;
}

enum StreamInner {
    Items(VecDeque<Result<ModelStreamItem, CoreError>>),
    Source(Box<dyn ModelStreamSource>),
}

/// Output of one streamed model call: text deltas, then one `Completed`.
///
/// Nothing is yielded after `Completed` or after an error.
pub struct ModelStream {
    inner: StreamInner,
    finished: bool,
}

impl ModelStream {
    pub fn completed(response: ModelResponse) -> Self {
        Self::from_items(vec![Ok(ModelStreamItem::Completed { response })])
    }

    pub fn from_items(items: Vec<Result<ModelStreamItem, CoreError>>) -> Self {
        Self {
            inner: StreamInner::Items(items.into()),
            finished: false,
        }
    }

    pub fn from_source(source: Box<dyn ModelStreamSource>) -> Self {
        Self {
            inner: StreamInner::Source(source),
            finished: false,
        }
    }

    pub async fn next(&mut self) -> Option<Result<ModelStreamItem, CoreError>> {
        if self.finished {
            return None;
        }
        let item = match &mut self.inner {
            StreamInner::Items(items) => items.pop_front(),
            StreamInner::Source(source) => source.next().await,
        };
        if !matches!(item, Some(Ok(ModelStreamItem::TextDelta { .. }))) {
            self.finished = true;
        }
        item
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stream_yields_nothing_after_completed_or_error() {
        let done = || {
            Ok(ModelStreamItem::Completed {
                response: ModelResponse {
                    content: None,
                    tool_calls: vec![],
                    usage: Default::default(),
                },
            })
        };
        let mut stream = ModelStream::from_items(vec![done(), done()]);
        assert!(matches!(
            stream.next().await,
            Some(Ok(ModelStreamItem::Completed { .. }))
        ));
        assert!(stream.next().await.is_none());

        let mut stream =
            ModelStream::from_items(vec![Err(CoreError::Model("boom".into())), done()]);
        assert!(matches!(
            stream.next().await,
            Some(Err(CoreError::Model(_)))
        ));
        assert!(stream.next().await.is_none());
    }
}
