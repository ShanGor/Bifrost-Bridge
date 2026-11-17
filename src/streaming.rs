//! Transport helpers shared by HTTP proxy paths. No detached body pump: downstream
//! backpressure controls upstream reads, and dropping the body cancels upstream.
use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use hyper::body::{Body, Frame, SizeHint};
use std::{
    error::Error,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::time::{Instant, Sleep};

pub(crate) type BoxError = Box<dyn Error + Send + Sync>;
pub(crate) type BoxedBody = BoxBody<Bytes, BoxError>;

pub(crate) async fn with_optional_timeout<F: Future>(
    duration: Duration,
    future: F,
) -> Result<F::Output, tokio::time::error::Elapsed> {
    if duration.is_zero() {
        Ok(future.await)
    } else {
        tokio::time::timeout(duration, future).await
    }
}

pub(crate) fn strip_hop_headers(headers: &mut http::HeaderMap, keep_upgrade: bool) {
    let nominated: Vec<_> = headers
        .get_all(http::header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|v| http::HeaderName::from_bytes(v.trim().as_bytes()).ok())
        .collect();
    for name in nominated {
        if !(keep_upgrade && name == http::header::UPGRADE) {
            headers.remove(name);
        }
    }
    for name in [
        "keep-alive",
        "proxy-connection",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
    ] {
        headers.remove(name);
    }
    if !keep_upgrade {
        headers.remove("connection");
        headers.remove("upgrade");
    }
}

/// Keeps accounting alive until EOF, failure or client cancellation. The idle
/// deadline measures upstream silence while the downstream is polling for data.
pub(crate) struct StreamingBody<G: Unpin> {
    inner: Option<BoxedBody>,
    guard: Option<G>,
    idle: Duration,
    deadline: Pin<Box<Sleep>>,
}

impl<G: Unpin> StreamingBody<G> {
    pub(crate) fn new(inner: BoxedBody, idle: Duration, guard: G) -> Self {
        Self {
            inner: Some(inner),
            guard: Some(guard),
            idle,
            deadline: Box::pin(tokio::time::sleep(idle)),
        }
    }
    fn finish(&mut self) {
        self.inner.take();
        self.guard.take();
    }
}

impl<G: Unpin> Body for StreamingBody<G> {
    type Data = Bytes;
    type Error = BoxError;
    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BoxError>>> {
        let this = self.get_mut();
        let Some(inner) = this.inner.as_mut() else {
            return Poll::Ready(None);
        };
        match Pin::new(inner).poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if frame.data_ref().is_some_and(|data| !data.is_empty()) {
                    this.deadline.as_mut().reset(Instant::now() + this.idle);
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(end) => {
                this.finish();
                Poll::Ready(end)
            }
            Poll::Pending => {
                if !this.idle.is_zero() && this.deadline.as_mut().poll(cx).is_ready() {
                    this.finish();
                    Poll::Ready(Some(Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "upstream SSE idle timeout",
                    )
                    .into())))
                } else {
                    Poll::Pending
                }
            }
        }
    }
    fn is_end_stream(&self) -> bool {
        self.inner.as_ref().is_none_or(|b| b.is_end_stream())
    }
    fn size_hint(&self) -> SizeHint {
        self.inner
            .as_ref()
            .map_or_else(SizeHint::default, |b| b.size_hint())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::{BodyExt, StreamBody};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    struct Guard(Arc<AtomicUsize>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn idle_timeout_releases_body_and_guard_and_is_terminal() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let body =
            StreamBody::new(futures::stream::pending::<Result<Frame<Bytes>, BoxError>>()).boxed();
        let mut body = StreamingBody::new(body, Duration::from_millis(20), Guard(dropped.clone()));
        let error = body.frame().await.unwrap().unwrap_err();
        assert!(error.to_string().contains("idle timeout"));
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        assert!(body.frame().await.is_none());
    }

    #[tokio::test]
    async fn accounting_survives_headers_and_releases_on_eof_or_cancellation() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let frames = futures::stream::iter([Ok::<_, BoxError>(Frame::data(Bytes::from_static(
            b": ping\n\n",
        )))]);
        let inner = StreamBody::new(futures::StreamExt::chain(
            frames,
            futures::stream::pending(),
        ))
        .boxed();
        let mut body = StreamingBody::new(inner, Duration::ZERO, Guard(dropped.clone()));
        body.frame().await.unwrap().unwrap();
        assert_eq!(dropped.load(Ordering::SeqCst), 0);
        drop(body);
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        let inner = http_body_util::Empty::<Bytes>::new()
            .map_err(|e| match e {})
            .boxed();
        let mut body = StreamingBody::new(inner, Duration::ZERO, Guard(dropped.clone()));
        assert!(body.frame().await.is_none());
        assert_eq!(dropped.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn heartbeat_resets_idle_deadline() {
        let stream = futures::stream::unfold(0, |count| async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            Some((
                Ok::<_, BoxError>(Frame::data(Bytes::from_static(b": ping\n\n"))),
                count + 1,
            ))
        });
        // Pin the asynchronous stream before boxing its body.
        let inner = StreamBody::new(Box::pin(stream)).boxed();
        let mut body = StreamingBody::new(inner, Duration::from_millis(100), ());
        for _ in 0..10 {
            assert!(body.frame().await.unwrap().is_ok());
        }
    }

    #[test]
    fn strips_all_connection_nominations_and_preserves_end_to_end_headers() {
        let mut headers = http::HeaderMap::new();
        headers.append("connection", "keep-alive, x-secret".parse().unwrap());
        headers.append("connection", "x-other".parse().unwrap());
        headers.insert("x-secret", "1".parse().unwrap());
        headers.insert("x-other", "2".parse().unwrap());
        headers.insert("last-event-id", "42".parse().unwrap());
        headers.insert("cache-control", "no-cache".parse().unwrap());
        strip_hop_headers(&mut headers, false);
        assert_eq!(headers.len(), 2);
        assert_eq!(headers["last-event-id"], "42");
    }
}
