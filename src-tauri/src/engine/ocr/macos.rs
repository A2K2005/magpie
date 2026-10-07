//! macOS Vision (VNRecognizeTextRequest). Adapted from system-ocr (MIT, github.com/Brooooooklyn/system-ocr).

use crate::types::OcrLine;
use image::DynamicImage;
use objc2::AnyThread;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::AnyObject;
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString};
use objc2_vision::{
    VNImageOption, VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel,
};

pub fn init_thread() {}

pub fn recognize(img: &DynamicImage) -> anyhow::Result<Vec<OcrLine>> {
    // Uncompressed BMP: ImageIO reads it, and encoding costs almost nothing.
    let mut bytes = Vec::new();
    img.to_rgb8().write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Bmp)?;
    unsafe {
        autoreleasepool(|pool| {
            let data = NSData::with_bytes(&bytes);
            let options: Retained<NSDictionary<VNImageOption, AnyObject>> = NSDictionary::new();
            let handler = VNImageRequestHandler::initWithData_options(VNImageRequestHandler::alloc(), &data, &options);
            let request = VNRecognizeTextRequest::init(VNRecognizeTextRequest::alloc());
            request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
            request.setUsesLanguageCorrection(true);
            request.setAutomaticallyDetectsLanguage(true);
            let langs: Retained<NSArray<NSString>> = NSArray::from_retained_slice(&[NSString::from_str("en-US")]);
            request.setRecognitionLanguages(&langs);
            let vn: Retained<VNRequest> = request.clone().into_super().into_super();
            handler
                .performRequests_error(&NSArray::from_retained_slice(&[vn]))
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            let mut out = Vec::new();
            for obs in request.results().unwrap_or_default() {
                let Some(best) = obs.topCandidates(1).firstObject() else { continue };
                let text = best.string().to_str(pool).trim().to_string();
                if text.is_empty() {
                    continue;
                }
                // Vision's origin is bottom-left.
                let bb = obs.boundingBox();
                out.push(OcrLine {
                    t: text,
                    x: bb.origin.x as f32,
                    y: (1.0 - bb.origin.y - bb.size.height) as f32,
                    w: bb.size.width as f32,
                    h: bb.size.height as f32,
                });
            }
            Ok(out)
        })
    }
}
