// SPDX-License-Identifier: MPL-2.0
//! A scripted upstream server that records what it receives.

use http_body_util::BodyExt;
use hyper::body::{Body, Bytes, Frame, Incoming};
use hyper::header::HeaderMap;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{Notify, mpsc};

/// One request as the upstream saw it.
#[derive(Clone)]
pub struct Seen {
    pub method: String,
    pub target: String,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

pub enum Step {
    Data(Vec<u8>),
    Wait(Arc<Notify>),
    Pause(u64),
}

pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, Vec<u8>)>,
    pub steps: Vec<Step>,
}

impl Reply {
    pub fn full(status: u16, headers: &[(&str, &[u8])], body: &[u8]) -> Reply {
        Reply::steps(status, headers, vec![Step::Data(body.to_vec())])
    }

    pub fn steps(status: u16, headers: &[(&str, &[u8])], steps: Vec<Step>) -> Reply {
        Reply {
            status,
            headers: headers
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.to_vec()))
                .collect(),
            steps,
        }
    }
}

struct ChannelBody(mpsc::Receiver<Bytes>);

impl Body for ChannelBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        self.0
            .poll_recv(cx)
            .map(|bytes| bytes.map(|b| Ok(Frame::data(b))))
    }
}

pub struct Mock {
    pub addr: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Mock {
    pub async fn start(handler: impl Fn(&Seen) -> Reply + Send + Sync + 'static) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let handler = Arc::new(handler);
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let (handler, log) = (handler.clone(), log.clone());
                let service = service_fn(move |req| respond(handler.clone(), log.clone(), req));
                tokio::spawn(async move {
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        Mock { addr, seen }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

async fn respond(
    handler: Arc<impl Fn(&Seen) -> Reply>,
    log: Arc<Mutex<Vec<Seen>>>,
    req: Request<Incoming>,
) -> Result<Response<ChannelBody>, Infallible> {
    let (parts, body) = req.into_parts();
    let seen = Seen {
        method: parts.method.to_string(),
        target: parts.uri.path_and_query().unwrap().to_string(),
        headers: parts.headers,
        body: body.collect().await.unwrap().to_bytes().to_vec(),
    };
    let reply = handler(&seen);
    log.lock().unwrap().push(seen);
    let (tx, rx) = mpsc::channel(8);
    tokio::spawn(async move {
        for step in reply.steps {
            match step {
                Step::Data(bytes) => {
                    if tx.send(Bytes::from(bytes)).await.is_err() {
                        return;
                    }
                }
                Step::Wait(notify) => notify.notified().await,
                Step::Pause(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
            }
        }
    });
    let mut response = Response::new(ChannelBody(rx));
    *response.status_mut() = StatusCode::from_u16(reply.status).unwrap();
    for (name, value) in reply.headers {
        response.headers_mut().insert(
            hyper::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            hyper::header::HeaderValue::from_bytes(&value).unwrap(),
        );
    }
    Ok(response)
}
