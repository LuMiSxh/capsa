// SPDX-License-Identifier: MPL-2.0
//! Passes a response body through frame by frame and reads its usage on the way.

use super::record::Pending;
use super::usage::{Usage, UsageTap};
use hyper::body::{Body, Bytes, Frame, SizeHint};
use std::pin::Pin;
use std::task::{Context, Poll};

type Upstream = reqwest::Body;

/// Forwards every frame as soon as the upstream yields it. The request is logged when the body
/// ends, fails or is dropped by a client that went away.
pub struct Tap {
    inner: Pin<Box<Upstream>>,
    usage: Option<UsageTap>,
    pending: Option<Pending>,
}

impl Tap {
    pub fn new(inner: Upstream, usage: Option<UsageTap>, pending: Pending) -> Self {
        Tap {
            inner: Box::pin(inner),
            usage,
            pending: Some(pending),
        }
    }

    fn settle(&mut self) {
        let usage = self
            .usage
            .take()
            .map_or_else(Usage::default, UsageTap::finish);
        if let Some(pending) = self.pending.take() {
            pending.finish(usage);
        }
    }
}

impl Body for Tap {
    type Data = Bytes;
    type Error = reqwest::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, reqwest::Error>>> {
        let this = self.get_mut();
        let polled = this.inner.as_mut().poll_frame(cx);
        match &polled {
            Poll::Ready(Some(Ok(frame))) => {
                if let (Some(tap), Some(data)) = (this.usage.as_mut(), frame.data_ref()) {
                    tap.feed(data);
                }
            }
            Poll::Ready(None | Some(Err(_))) => this.settle(),
            Poll::Pending => {}
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        self.settle();
    }
}
