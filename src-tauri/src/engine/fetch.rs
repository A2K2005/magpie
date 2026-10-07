//! Model downloads: each file once, to `<name>.part` then renamed, so a cut connection never leaves
//! a truncated model behind.

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

pub fn valid(file: &Path, name: &str) -> bool {
    let Ok(meta) = file.metadata() else {
        return false;
    };
    if !meta.is_file() || meta.len() == 0 {
        return false;
    }
    let Some((_, expected)) = super::clip::SHA256.iter().chain(super::ocr::paddle::SHA256.iter()).find(|(f, _)| *f == name) else {
        return true;
    };
    let Ok(mut input) = std::fs::File::open(file) else {
        return false;
    };
    let mut hash = Sha256::new();
    let mut buf = [0u8; 1 << 16];
    loop {
        let Ok(n) = input.read(&mut buf) else {
            return false;
        };
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    format!("{:x}", hash.finalize()) == *expected
}

/// Downloads the missing files of `(file in dir, URL, approximate size)`. `progress` gets 0-1.
pub fn download(
    dir: &Path,
    files: &[(&str, &str, u64)],
    progress: &dyn Fn(f64),
) -> anyhow::Result<()> {
    let mut sizes: Vec<u64> = files.iter().map(|f| f.2).collect();
    let mut done_before = 0u64;
    for (i, (file, url, _)) in files.iter().enumerate() {
        let dest = dir.join(file);
        if valid(&dest, file) {
            done_before += sizes[i];
            continue;
        }
        std::fs::create_dir_all(dest.parent().unwrap())?;
        let mut res = ureq::get(*url).call()?;
        let expected_len = res.body().content_length();
        if let Some(len) = expected_len {
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
        anyhow::ensure!(
            got > 0 && expected_len.is_none_or(|n| got == n),
            "Incomplete model download: {file}"
        );
        anyhow::ensure!(valid(&part, file), "Model checksum mismatch: {file}");
        // Keep invalid caches until a verified replacement exists.
        if dest.exists() {
            std::fs::remove_file(&dest)?;
        }
        std::fs::rename(&part, &dest)?;
        done_before += got;
    }
    progress(1.0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_corrupt_pinned_model_and_empty_other_files() {
        let p = std::env::temp_dir().join(format!("glint-model-test-{}", std::process::id()));
        std::fs::write(&p, b"corrupt").unwrap();
        assert!(!valid(&p, "tokenizer.json"));
        assert!(!valid(&p, "paddle/dict.txt"));
        assert!(valid(&p, "unlisted-file"));
        std::fs::write(&p, b"").unwrap();
        assert!(!valid(&p, "paddle/dict.txt"));
        std::fs::remove_file(p).unwrap();
    }
}
