use std::thread;
use std::time::Duration;

pub const FLUSH_AT: usize = 32 * 1024;

#[derive(Clone, Copy, Debug, Default)]
pub struct Summary {
    pub ok: u64,
    pub fail: u64,
}

pub fn concurrency() -> usize {
    thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .max(1)
}

pub fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(120))
        .pool_max_idle_per_host(128)
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_nodelay(true)
        .build()
        .map_err(|err| err.to_string())
}

/// pull a short response off the socket and drop it.
/// the bytes are not inspected. anything past `cap` is abandoned.
/// redirects are never followed.
pub async fn discard_unread(mut resp: reqwest::Response, cap: usize) {
    let mut seen = 0usize;
    while seen < cap {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                seen = seen.saturating_add(chunk.len());
                drop(chunk);
            }
            _ => return,
        }
    }
}

pub fn brief(err: &str) -> String {
    let mut end = err.len().min(120);
    while end > 0 && !err.is_char_boundary(end) {
        end -= 1;
    }
    if end == err.len() {
        err.to_string()
    } else {
        format!("{}...", &err[..end])
    }
}

pub fn random_hex32() -> [u8; 32] {
    let mut raw = [0u8; 16];
    fill_random(&mut raw);
    let mut dst = [0u8; 32];
    write_hex(&raw, &mut dst);
    dst
}

fn fill_random(dest: &mut [u8]) {
    #[cfg(not(any(unix, windows)))]
    compile_error!("rng is implemented for unix and windows");
    if dest.is_empty() {
        return;
    }
    #[cfg(unix)]
    {
        use std::fs::File;
        use std::io::Read;
        let mut file = File::open("/dev/urandom").expect("rng");
        file.read_exact(dest).expect("rng");
    }
    #[cfg(windows)]
    {
        #[link(name = "advapi32")]
        extern "system" {
            fn SystemFunction036(random_buffer: *mut u8, random_buffer_length: u32) -> u8;
        }
        // SAFETY: dest is writable for dest.len() bytes. SystemFunction036 (RtlGenRandom) fills that many.
        let ok = unsafe { SystemFunction036(dest.as_mut_ptr(), dest.len() as u32) };
        assert!(ok != 0, "rng");
    }
}

fn write_hex(raw: &[u8; 16], dst: &mut [u8; 32]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut i = 0;
    while i < 16 {
        let b = raw[i];
        dst[i * 2] = HEX[(b >> 4) as usize];
        dst[i * 2 + 1] = HEX[(b & 0x0f) as usize];
        i += 1;
    }
}

pub fn rand_u64() -> u64 {
    let mut raw = [0u8; 8];
    fill_random(&mut raw);
    u64::from_ne_bytes(raw)
}

pub fn push_token(dst: &mut String) {
    let mut raw = [0u8; 32];
    fill_random(&mut raw);
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    dst.reserve(43);
    let mut i = 0;
    while i + 2 < raw.len() {
        let n = (u32::from(raw[i]) << 16) | (u32::from(raw[i + 1]) << 8) | u32::from(raw[i + 2]);
        dst.push(T[((n >> 18) & 63) as usize] as char);
        dst.push(T[((n >> 12) & 63) as usize] as char);
        dst.push(T[((n >> 6) & 63) as usize] as char);
        dst.push(T[(n & 63) as usize] as char);
        i += 3;
    }
    let n = (u32::from(raw[30]) << 16) | (u32::from(raw[31]) << 8);
    dst.push(T[((n >> 18) & 63) as usize] as char);
    dst.push(T[((n >> 12) & 63) as usize] as char);
    dst.push(T[((n >> 6) & 63) as usize] as char);
}
