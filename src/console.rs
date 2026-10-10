use crate::util::FLUSH_AT;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncWriteExt, Stdout};
use tokio::sync::mpsc::{self, Receiver, Sender};

enum Msg {
    Line(String),
    Shut,
}

#[derive(Clone)]
pub struct Log {
    tx: Sender<Msg>,
    dropped: Arc<AtomicU64>,
}

impl Log {
    pub fn line(&self, text: impl AsRef<str>) {
        if self.tx.try_send(Msg::Line(with_newline(text.as_ref()))).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub async fn line_wait(&self, text: &str) {
        let _ = self.tx.send(Msg::Line(with_newline(text))).await;
    }

    pub fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub async fn shutdown(self) {
        let _ = self.tx.send(Msg::Shut).await;
    }
}

pub fn start() -> (Log, tokio::task::JoinHandle<()>) {
    let (tx, rx) = mpsc::channel(1024);
    let log = Log {
        tx,
        dropped: Arc::new(AtomicU64::new(0)),
    };
    let worker = tokio::spawn(consume(rx));
    (log, worker)
}

fn with_newline(text: &str) -> String {
    let mut line = String::with_capacity(text.len() + 1);
    line.push_str(text);
    if !line.ends_with('\n') {
        line.push('\n');
    }
    line
}

async fn consume(mut rx: Receiver<Msg>) {
    let mut out = tokio::io::stdout();
    let mut buf = Vec::<u8>::with_capacity(FLUSH_AT * 2);
    loop {
        let msg = match rx.recv().await {
            Some(msg) => msg,
            None => break,
        };
        if handle(&mut buf, &mut out, msg).await {
            return;
        }
        while let Ok(msg) = rx.try_recv() {
            if handle(&mut buf, &mut out, msg).await {
                return;
            }
        }
        flush(&mut out, &mut buf).await;
    }
    flush(&mut out, &mut buf).await;
}

async fn handle(buf: &mut Vec<u8>, out: &mut Stdout, msg: Msg) -> bool {
    match msg {
        Msg::Shut => {
            flush(out, buf).await;
            true
        }
        Msg::Line(line) => {
            buf.extend_from_slice(line.as_bytes());
            if buf.len() >= FLUSH_AT {
                flush(out, buf).await;
            }
            false
        }
    }
}

async fn flush(out: &mut Stdout, buf: &mut Vec<u8>) {
    if buf.is_empty() {
        return;
    }
    let _ = out.write_all(buf).await;
    let _ = out.flush().await;
    buf.clear();
}
