use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

fn compress(src: &str, dest_name: &str, level: u32) {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src_path = manifest.join(src);
    println!("cargo:rerun-if-changed={src}");
    let raw = fs::read(&src_path).unwrap_or_else(|err| {
        panic!("read {}: {err}", src_path.display());
    });
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(level));
    enc.write_all(&raw)
        .unwrap_or_else(|err| panic!("compress {src}: {err}"));
    let gz = enc.finish().unwrap_or_else(|err| panic!("finish {src}: {err}"));
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join(dest_name);
    fs::write(&out, gz).unwrap_or_else(|err| panic!("write {}: {err}", out.display()));
}

fn main() {
    compress("samples/json-1mb.json", "json-1mb.json.gz", 6);
    compress("samples/image.png", "image.png.gz", 1);
}
