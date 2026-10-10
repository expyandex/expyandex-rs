use flate2::read::GzDecoder;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub const JSON_NAME: &str = "json-1mb.json";
pub const IMAGE_NAME: &str = "image.png";

pub const JSON_GZ: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/json-1mb.json.gz"));
pub const IMAGE_GZ: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/image.png.gz"));

pub struct Unpacked {
    pub json_path: PathBuf,
    pub image_path: PathBuf,
    pub json_len: u64,
    pub image_len: u64,
    pub json_wrote: bool,
    pub image_wrote: bool,
}

pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(PathBuf::from))
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn unpack(dir: &Path, replace: bool) -> Result<Unpacked, String> {
    let json_path = dir.join(JSON_NAME);
    let image_path = dir.join(IMAGE_NAME);
    let (json_wrote, json_len) = ensure(&json_path, JSON_GZ, replace)?;
    let (image_wrote, image_len) = ensure(&image_path, IMAGE_GZ, replace)?;
    Ok(Unpacked {
        json_path,
        image_path,
        json_len,
        image_len,
        json_wrote,
        image_wrote,
    })
}

pub fn inflate(gz: &[u8]) -> io::Result<Vec<u8>> {
    let mut dec = GzDecoder::new(gz);
    let mut out = Vec::new();
    dec.read_to_end(&mut out)?;
    Ok(out)
}

fn ensure(path: &Path, gz: &[u8], replace: bool) -> Result<(bool, u64), String> {
    let missing = !path.is_file() || file_len(path).unwrap_or(0) == 0;
    let wrote = replace || missing;
    if wrote {
        let bytes = inflate(gz).map_err(|err| format!("{}: {err}", path.display()))?;
        write_bytes(path, &bytes).map_err(|err| format!("{}: {err}", path.display()))?;
    }
    let len = file_len(path).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok((wrote, len))
}

fn file_len(path: &Path) -> io::Result<u64> {
    Ok(fs::metadata(path)?.len())
}

fn write_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let tmp = path.with_extension("part");
    {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    if fs::rename(&tmp, path).is_err() {
        if path.exists() {
            fs::remove_file(path)?;
        }
        fs::rename(&tmp, path)?;
    }
    Ok(())
}
