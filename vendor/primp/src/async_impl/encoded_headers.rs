use http::header::CONTENT_ENCODING;
use http::{HeaderMap, Request, Response};
use pin_project_lite::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use tower::Service;

/// Response headers before the decompression layer changes them.
#[derive(Clone, Debug)]
pub(crate) struct EncodedHeaders(pub HeaderMap);

#[derive(Clone, Debug)]
pub(crate) struct CaptureEncodedHeaders<S> {
    inner: S,
}

impl<S> CaptureEncodedHeaders<S> {
    pub(crate) fn new(inner: S) -> Self {
        Self { inner }
    }
}

pin_project! {
    pub(crate) struct CaptureFuture<F> {
        #[pin]
        inner: F,
    }
}

impl<F, B, E> Future for CaptureFuture<F>
where
    F: Future<Output = Result<Response<B>, E>>,
{
    type Output = Result<Response<B>, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.project().inner.poll(cx) {
            Poll::Ready(Ok(mut response)) => {
                if response.headers().contains_key(CONTENT_ENCODING) {
                    let headers = EncodedHeaders(response.headers().clone());
                    response.extensions_mut().insert(headers);
                }
                Poll::Ready(Ok(response))
            }
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for CaptureEncodedHeaders<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = CaptureFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<ReqBody>) -> Self::Future {
        CaptureFuture { inner: self.inner.call(request) }
    }
}
