//! Model downloads: each file once, to `<name>.part` then renamed, so a cut connection never leaves
//! a truncated model behind.

use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

/// Downloads the missing files of `(file in dir, URL, approximate size)`. `progress` gets 0-1.
pub fn download(dir: &Path, files: &[(&str, &str, u64)], progress: &dyn Fn(f64)) -> anyhow::Result<()> {
    let mut sizes: Vec<u64> = files.iter().map(|f| f.2).collect();
    let mut done_before = 0u64;
    for (i, (file, url, _)) in files.iter().enumerate() {
        let dest = dir.join(file);
        if dest.exists() {
            done_before += sizes[i];
            continue;
        }
        std::fs::create_dir_all(dest.parent().unwrap())?;
        let mut res = ureq::get(*url).call()?;
        if let Some(len) = res.body().content_length() {
            sizes[i] = len;
        }
        let total: u64 = sizes.iter().sum();
        let part = dest.with_extension("part");
        let mut out = std::fs::File::create(&part)?;
        let mut reader = res.body_mut().as_reader();
        let mut buf = vec![0u8; 1 << 16];
        let mut got = 0u64;
        let mut last = Instant::now();
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])?;
            got += n as u64;
            if last.elapsed() > Duration::from_millis(200) {
                progress((done_before + got) as f64 / total as f64);
                last = Instant::now();
            }
        }
        drop(out);
        std::fs::rename(&part, &dest)?;
        done_before += got;
    }
    progress(1.0);
    Ok(())
}
