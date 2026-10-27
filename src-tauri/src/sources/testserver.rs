//! A minimal local HTTP/1.1 server for fetcher tests: real sockets, scripted replies, and
//! counters for hits, concurrency and request start times.

use std::{
    fmt::Write as _,
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};

#[derive(Debug, Clone)]
pub struct Req {
    /// 1-based sequence number of this request across the whole server.
    pub hit: usize,
    pub path: String,
    pub headers: Vec<(String, String)>,
}

impl Req {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Wait this long before answering (simulates a slow server).
    pub delay: Duration,
}

impl Reply {
    pub fn ok(body: &str) -> Self {
        Self::status(200).body(body.as_bytes())
    }

    pub fn status(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
            delay: Duration::ZERO,
        }
    }

    pub fn body(mut self, body: &[u8]) -> Self {
        self.body = body.to_vec();
        self
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    pub fn redirect(location: &str) -> Self {
        Self::status(302).header("Location", location)
    }
}

#[derive(Default)]
pub struct Counters {
    hits: AtomicUsize,
    active: AtomicUsize,
    max_active: AtomicUsize,
    starts: Mutex<Vec<Instant>>,
    paths: Mutex<Vec<String>>,
}

pub struct TestServer {
    addr: SocketAddr,
    counters: Arc<Counters>,
    task: JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

type Handler = Arc<dyn Fn(&Req) -> Reply + Send + Sync>;

impl TestServer {
    pub async fn start(handler: impl Fn(&Req) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let counters = Arc::new(Counters::default());
        let handler: Handler = Arc::new(handler);
        let task = {
            let counters = Arc::clone(&counters);
            tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        break;
                    };
                    tokio::spawn(serve(stream, Arc::clone(&handler), Arc::clone(&counters)));
                }
            })
        };
        Self {
            addr,
            counters,
            task,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    pub fn hits(&self) -> usize {
        self.counters.hits.load(Ordering::SeqCst)
    }

    pub fn max_concurrent(&self) -> usize {
        self.counters.max_active.load(Ordering::SeqCst)
    }

    pub fn start_times(&self) -> Vec<Instant> {
        self.counters.starts.lock().unwrap().clone()
    }

    pub fn paths(&self) -> Vec<String> {
        self.counters.paths.lock().unwrap().clone()
    }
}

async fn serve(mut stream: TcpStream, handler: Handler, counters: Arc<Counters>) {
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        if buf.len() > 64 * 1024 {
            return;
        }
    }
    let head = String::from_utf8_lossy(&buf).into_owned();
    let mut lines = head.split("\r\n");
    let path = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_owned();
    let headers = lines
        .take_while(|l| !l.is_empty())
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();

    let hit = counters.hits.fetch_add(1, Ordering::SeqCst) + 1;
    let now_active = counters.active.fetch_add(1, Ordering::SeqCst) + 1;
    counters.max_active.fetch_max(now_active, Ordering::SeqCst);
    counters.starts.lock().unwrap().push(Instant::now());
    counters.paths.lock().unwrap().push(path.clone());

    let reply = handler(&Req { hit, path, headers });
    tokio::time::sleep(reply.delay).await;

    let mut out = format!(
        "HTTP/1.1 {} Test\r\nContent-Length: {}\r\nConnection: close\r\n",
        reply.status,
        reply.body.len()
    );
    for (k, v) in &reply.headers {
        let _ = write!(out, "{k}: {v}\r\n");
    }
    out.push_str("\r\n");
    let _ = stream.write_all(out.as_bytes()).await;
    for part in reply.body.chunks(8 * 1024) {
        if stream.write_all(part).await.is_err() {
            break;
        }
    }
    let _ = stream.shutdown().await;
    counters.active.fetch_sub(1, Ordering::SeqCst);
}
