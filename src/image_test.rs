use crate::console::Log;
use crate::util::{brief, concurrency, discard_unread, random_hex32, Summary};
use bytes::Bytes;
use reqwest::header::{
    HeaderMap, HeaderName, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, COOKIE, ORIGIN,
    REFERER, SET_COOKIE, USER_AGENT,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::sync::{watch, Semaphore};
use tokio::task::JoinSet;
use tokio::time::{sleep, Duration};

const UPLOAD_URL: &str = "https://forms.yandex.com/gateway/root/form/uploadFile";
const SURVEY_PAGE: &str = "https://forms.yandex.com/surveys/2182/?lang=en";
const FILE_NAME: &str = "image.png";
const SURVEY_ID: &str = "2182";
const MAX_SESSION_ATTEMPTS: u32 = 3;

const EMBEDDED_COOKIES: &[(&str, &str)] = &[
    (
        "CSRF-TOKEN",
        "3cd15f3a95c782471c2a39081ab36462368671c1%3A1779336349",
    ),
    ("forms_sid", "vvZrvUL9CpL2eMLx"),
    ("yandexuid", "497216651774654215"),
    ("yuidss", "497216651774654215"),
    (
        "i",
        "bV4PkcMLSLYcxBxRTgcHSqQ0jmwh+/y5ebKwdczNripGMeTO4qu4ZZhq6FV5FibV1CmRj1g+1oWyA8GinXsBlqttU04=",
    ),
    ("yashr", "4506242121779331516"),
    (
        "pi",
        "94x6JjTUcXEnNBztZxsVNqFZcAz63w/eYJ1eGntiWpkAGIcVi8pfdCpqxexUshp79C6eI+lm7a3y1btzhSc0fRZowIE=",
    ),
    (
        "spravka",
        "dD0xNzc5MzM1Mzk2O2k9NzMuNTkuMTc0LjIzODtEPUJDNzE2QUZGM0YwREQzMTI0MjUzOEE1NUQ0ODYyM0UyODQxN0FCNzU0OUEyMDI2REIzQkEwNzk4ODU0QjE0MzIyODFDRTY5RTZDMjU3NDAwMUQ3N0RDNDZCODZEOUIyRTcxQTJDQzAyNDdDQ0Y0OTIxMUMwOEZEMDdBRjJFREUzMjdFNkQ1Rjg5MzZBRTAxMEI0NzQ0NzE0M0FEMzRDODk0MEEwQ0JBMUY0RENEMkRGOEVDQzdFO3U9MTc3OTMzNTM5NjU5ODQ1NjY1OTtoPTlmNGVhNGYzMGRhOGE3MjBiM2NkODEyY2QxNTk3NTVh",
    ),
    (
        "_yasc",
        "GxQQOH2C2fGEOR9UFDBuXq5EJyNrnYxUBTeat9gG0CCl+Nh2ZzcvcFc5CW9C4tOq/R9zi2indJZRr4Yh6cDZs/IfXmxdFfg=",
    ),
    (
        "bh",
        "EkEiQ2hyb21pdW0iO3Y9IjE0OCIsICJHb29nbGUgQ2hyb21lIjt2PSIxNDgiLCAiTm90L0EpQnJhbmQiO3Y9Ijk5IhoDeDg2Ig4xNDguMC43Nzc4LjE2OCoCPzAyAiIiOgkiV2luZG93cyJCBjE5LjAuMEoCNjRSWyJDaHJvbWl1bSI7dj0iMTQ4LjAuNzc3OC4xNjgiLCJHb29nbGUgQ2hyb21lIjt2PSIxNDguMC43Nzc4LjE2OCIsIk5vdC9BKUJyYW5kIjt2PSI5OS4wLjAuMCJaAj8wYJ2JutAGah7cyuH/CJLYobEDn8/h6gP7+vDnDev//fYP06DOhwg=",
    ),
    ("gdpr", "0"),
    ("is_gdpr", "0"),
    ("cookieon", "1"),
];

pub async fn run(
    client: reqwest::Client,
    png: Arc<Vec<u8>>,
    n: u64,
    stop: watch::Receiver<bool>,
    log: Log,
) -> Summary {
    let (body, content_type) = multipart(&png);
    let conc = concurrency();
    let gate = Arc::new(Semaphore::new(1));
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
        let body = body.clone();
        let content_type = content_type.clone();
        let gate = Arc::clone(&gate);
        let mut stop_r = stop.clone();
        let log = log.clone();
        let ok = Arc::clone(&ok);
        let fail = Arc::clone(&fail);
        set.spawn(async move {
            tokio::select! {
                _ = stop_r.wait_for(|flag| *flag) => {}
                hit = once(&client, &gate, body, &content_type, attempt) => {
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

struct Session {
    csrf: String,
    uid: String,
    cookie: String,
}

async fn once(
    client: &reqwest::Client,
    gate: &Semaphore,
    body: Bytes,
    content_type: &HeaderValue,
    attempt: u64,
) -> Hit {
    let session = {
        let _permit = match gate.acquire().await {
            Ok(permit) => permit,
            Err(_) => {
                return Hit {
                    ok: false,
                    line: format!("image #{attempt} fail session closed"),
                };
            }
        };
        match establish_session(client).await {
            Ok(session) => session,
            Err(err) => {
                return Hit {
                    ok: false,
                    line: format!("image #{attempt} fail {}", brief(&err)),
                };
            }
        }
    };

    let headers = match upload_headers(&session, content_type) {
        Ok(headers) => headers,
        Err(err) => {
            return Hit {
                ok: false,
                line: format!("image #{attempt} fail {}", brief(&err)),
            };
        }
    };

    match client
        .post(UPLOAD_URL)
        .headers(headers)
        .body(body)
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status().as_u16();
            if resp.status().is_success() {
                let url = read_url(resp).await;
                if let Some(url) = url {
                    Hit {
                        ok: true,
                        line: format!("image #{attempt} ok {status} {url}"),
                    }
                } else {
                    Hit {
                        ok: false,
                        line: format!("image #{attempt} fail {status}"),
                    }
                }
            } else {
                discard_unread(resp, 4096).await;
                Hit {
                    ok: false,
                    line: format!("image #{attempt} fail {status}"),
                }
            }
        }
        Err(err) => Hit {
            ok: false,
            line: format!("image #{attempt} fail {}", brief(&err.to_string())),
        },
    }
}

async fn establish_session(client: &reqwest::Client) -> Result<Session, String> {
    let mut jar = seed_jar();
    let mut last = String::from("session failed");
    for attempt in 1..=MAX_SESSION_ATTEMPTS {
        if attempt > 1 {
            sleep(Duration::from_millis(50 * u64::from(attempt))).await;
        }
        match fetch_session(client, &mut jar).await {
            Ok(session) => return Ok(session),
            Err(err) => last = err,
        }
    }
    Err(last)
}

async fn fetch_session(
    client: &reqwest::Client,
    jar: &mut Vec<(String, String)>,
) -> Result<Session, String> {
    let cookie = cookie_header(jar);
    let headers = nav_headers(&cookie)?;
    let resp = client
        .get(SURVEY_PAGE)
        .headers(headers)
        .send()
        .await
        .map_err(|err| err.to_string())?;
    let status = resp.status().as_u16();
    ingest_set_cookie(resp.headers(), jar);
    discard_unread(resp, 4096).await;
    if !(200..300).contains(&status) {
        return Err(format!("survey {status}"));
    }
    let csrf = jar_get(jar, "CSRF-TOKEN").ok_or("no csrf")?.to_string();
    let uid = jar_get(jar, "yandexuid").ok_or("no yandexuid")?.to_string();
    Ok(Session {
        csrf: percent_decode(&csrf),
        uid,
        cookie: cookie_header(jar),
    })
}

fn seed_jar() -> Vec<(String, String)> {
    EMBEDDED_COOKIES
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

fn ingest_set_cookie(headers: &HeaderMap, jar: &mut Vec<(String, String)>) {
    for value in headers.get_all(SET_COOKIE) {
        let Ok(text) = value.to_str() else {
            continue;
        };
        let Some(pair) = text.split(';').next() else {
            continue;
        };
        let Some((name, val)) = pair.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        upsert(jar, name, val.trim());
    }
}

fn upsert(jar: &mut Vec<(String, String)>, name: &str, value: &str) {
    if let Some((_, slot)) = jar.iter_mut().find(|(k, _)| k == name) {
        slot.clear();
        slot.push_str(value);
    } else {
        jar.push((name.to_string(), value.to_string()));
    }
}

fn jar_get<'a>(jar: &'a [(String, String)], name: &str) -> Option<&'a str> {
    jar.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

fn cookie_header(jar: &[(String, String)]) -> String {
    let mut s = String::with_capacity(2048);
    for (i, (name, value)) in jar.iter().enumerate() {
        if i > 0 {
            s.push_str("; ");
        }
        s.push_str(name);
        s.push('=');
        s.push_str(value);
    }
    s
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn nav_headers(cookie: &str) -> Result<HeaderMap, String> {
    let mut headers = base_nav_headers().clone();
    headers.insert(
        COOKIE,
        HeaderValue::from_str(cookie).map_err(|err| err.to_string())?,
    );
    Ok(headers)
}

fn upload_headers(session: &Session, content_type: &HeaderValue) -> Result<HeaderMap, String> {
    let mut headers = base_upload_headers().clone();
    headers.insert(CONTENT_TYPE, content_type.clone());
    headers.insert(
        COOKIE,
        HeaderValue::from_str(&session.cookie).map_err(|err| err.to_string())?,
    );
    headers.insert(
        HeaderName::from_static("x-csrf-token"),
        HeaderValue::from_str(&session.csrf).map_err(|err| err.to_string())?,
    );
    headers.insert(
        HeaderName::from_static("x-forms-yandexuid"),
        HeaderValue::from_str(&session.uid).map_err(|err| err.to_string())?,
    );
    Ok(headers)
}

fn base_nav_headers() -> &'static HeaderMap {
    static HEADERS: OnceLock<HeaderMap> = OnceLock::new();
    HEADERS.get_or_init(|| {
        let mut h = HeaderMap::with_capacity(12);
        h.insert(
            ACCEPT,
            HeaderValue::from_static(
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            ),
        );
        h.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en"));
        h.insert(
            USER_AGENT,
            HeaderValue::from_static(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/144.0.0.0 Safari/537.36",
            ),
        );
        insert_static(
            &mut h,
            "sec-ch-ua",
            "\"Chromium\";v=\"144\", \"Google Chrome\";v=\"144\", \"Not/A)Brand\";v=\"24\"",
        );
        insert_static(&mut h, "sec-ch-ua-mobile", "?0");
        insert_static(&mut h, "sec-ch-ua-platform", "\"Windows\"");
        insert_static(&mut h, "sec-fetch-dest", "document");
        insert_static(&mut h, "sec-fetch-mode", "navigate");
        insert_static(&mut h, "sec-fetch-site", "none");
        insert_static(&mut h, "sec-fetch-user", "?1");
        insert_static(&mut h, "upgrade-insecure-requests", "1");
        h
    })
}

fn base_upload_headers() -> &'static HeaderMap {
    static HEADERS: OnceLock<HeaderMap> = OnceLock::new();
    HEADERS.get_or_init(|| {
        let mut h = HeaderMap::with_capacity(12);
        h.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/plain, */*"),
        );
        h.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en"));
        h.insert(ORIGIN, HeaderValue::from_static("https://forms.yandex.com"));
        h.insert(REFERER, HeaderValue::from_static(SURVEY_PAGE));
        h.insert(
            USER_AGENT,
            HeaderValue::from_static(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/144.0.0.0 Safari/537.36",
            ),
        );
        insert_static(
            &mut h,
            "sec-ch-ua",
            "\"Chromium\";v=\"144\", \"Google Chrome\";v=\"144\", \"Not/A)Brand\";v=\"24\"",
        );
        insert_static(&mut h, "sec-ch-ua-mobile", "?0");
        insert_static(&mut h, "sec-ch-ua-platform", "\"Windows\"");
        insert_static(&mut h, "sec-fetch-dest", "empty");
        insert_static(&mut h, "sec-fetch-mode", "cors");
        insert_static(&mut h, "sec-fetch-site", "same-origin");
        h
    })
}

fn insert_static(headers: &mut HeaderMap, name: &'static str, value: &'static str) {
    headers.insert(HeaderName::from_static(name), HeaderValue::from_static(value));
}

fn multipart(png: &[u8]) -> (Bytes, HeaderValue) {
    let hex = random_hex32();
    let hex_str = std::str::from_utf8(&hex).unwrap();
    let boundary = format!("expy{hex_str}");
    let mut body = Vec::with_capacity(png.len() + 320);
    body.extend_from_slice(b"--");
    body.extend_from_slice(boundary.as_bytes());
    body.extend_from_slice(
        format!(
            "\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{FILE_NAME}\"\r\nContent-Type: image/png\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(png);
    body.extend_from_slice(b"\r\n--");
    body.extend_from_slice(boundary.as_bytes());
    body.extend_from_slice(
        format!(
            "\r\nContent-Disposition: form-data; name=\"surveyId\"\r\n\r\n{SURVEY_ID}\r\n--"
        )
        .as_bytes(),
    );
    body.extend_from_slice(boundary.as_bytes());
    body.extend_from_slice(b"--\r\n");
    let content_type = HeaderValue::from_str(&format!("multipart/form-data; boundary={boundary}"))
        .expect("multipart content type");
    (Bytes::from(body), content_type)
}

/// copy the upload `url` string out of a small prefix of the body.
/// the url is not requested.
async fn read_url(mut resp: reqwest::Response) -> Option<String> {
    const CAP: usize = 16 * 1024;
    let mut buf = Vec::with_capacity(512);
    while buf.len() < CAP {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                let room = CAP - buf.len();
                if chunk.len() <= room {
                    buf.extend_from_slice(&chunk);
                } else {
                    buf.extend_from_slice(&chunk[..room]);
                    break;
                }
            }
            _ => break,
        }
    }
    extract_url(&buf)
}

fn extract_url(body: &[u8]) -> Option<String> {
    const KEY: &[u8] = b"\"url\"";
    let mut i = 0usize;
    while i + KEY.len() <= body.len() {
        if &body[i..i + KEY.len()] == KEY {
            let mut j = i + KEY.len();
            while j < body.len() && matches!(body[j], b' ' | b'\t' | b'\n' | b'\r' | b':') {
                j += 1;
            }
            if j >= body.len() || body[j] != b'"' {
                i += 1;
                continue;
            }
            j += 1;
            let mut url = Vec::new();
            while j < body.len() && body[j] != b'"' {
                if body[j] == b'\\' {
                    if j + 1 < body.len() && body[j + 1] == b'/' {
                        url.push(b'/');
                        j += 2;
                        continue;
                    }
                    return None;
                }
                url.push(body[j]);
                j += 1;
            }
            if j >= body.len() || body[j] != b'"' {
                return None;
            }
            if url.iter().any(|b| *b <= 0x20) {
                return None;
            }
            let text = std::str::from_utf8(&url).ok()?;
            if text.starts_with("https://") || text.starts_with("http://") {
                return Some(text.to_string());
            }
            return None;
        }
        i += 1;
    }
    None
}
