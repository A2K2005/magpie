//! Windows.Media.Ocr. Adapted from system-ocr (MIT, github.com/Brooooooklyn/system-ocr).

use crate::types::OcrLine;
use image::{DynamicImage, GenericImageView};
use std::cell::RefCell;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;

thread_local! {
    static ENGINE: RefCell<Option<OcrEngine>> = const { RefCell::new(None) };
}

/// WinRT needs the calling thread in an apartment.
pub fn init_thread() {
    unsafe {
        let _ = windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED);
    }
}

fn engine() -> windows::core::Result<OcrEngine> {
    ENGINE.with(|e| {
        if let Some(engine) = e.borrow().as_ref() {
            return Ok(engine.clone());
        }
        // Uses the languages of the user's Windows profile; fails if none has an OCR pack.
        let engine = OcrEngine::TryCreateFromUserProfileLanguages()?;
        *e.borrow_mut() = Some(engine.clone());
        Ok(engine)
    })
}

pub fn recognize(img: &DynamicImage) -> anyhow::Result<Vec<OcrLine>> {
    let (w, h) = img.dimensions();
    let max = OcrEngine::MaxImageDimension()?;
    if w > max || h > max {
        return recognize(&img.resize(max, max, image::imageops::FilterType::Triangle));
    }
    let mut bgra = img.to_rgba8().into_raw();
    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let writer = DataWriter::new()?;
    writer.WriteBytes(&bgra)?;
    let buffer = writer.DetachBuffer()?;
    drop(bgra);
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, w as i32, h as i32)?;
    let result = engine()?.RecognizeAsync(&bitmap)?.join()?;

    let (fw, fh) = (w as f32, h as f32);
    let native = result.Lines()?;
    let mut out = Vec::with_capacity(native.Size()? as usize);
    for line in native {
        let words = line.Words()?;
        let (mut l, mut t, mut r, mut b) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for word in words {
            let rc = word.BoundingRect()?;
            l = l.min(rc.X);
            t = t.min(rc.Y);
            r = r.max(rc.X + rc.Width);
            b = b.max(rc.Y + rc.Height);
        }
        if l > r {
            continue;
        }
        out.push(OcrLine {
            t: line.Text()?.to_string_lossy().trim().to_string(),
            x: (l / fw).clamp(0.0, 1.0),
            y: (t / fh).clamp(0.0, 1.0),
            w: ((r - l) / fw).clamp(0.0, 1.0),
            h: ((b - t) / fh).clamp(0.0, 1.0),
        });
    }
    Ok(out)
}
