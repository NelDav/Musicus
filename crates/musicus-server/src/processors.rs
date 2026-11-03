//! Format processors run once per upload. The blob store does not care
//! about formats; processors recognise a file and extract what the library
//! needs. Only PDF exists today; MusicXML would be a second processor.

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings, render};

pub const PDF_MIME: &str = "application/pdf";

/// Thumbnails are rendered this wide, in pixels.
const THUMBNAIL_WIDTH: f32 = 240.0;

pub struct Processed {
    pub mime: &'static str,
    pub page_count: Option<i64>,
    pub thumbnail_png: Option<Vec<u8>>,
}

/// Returns `None` if the file is not of a supported format.
pub fn sniff_mime(head: &[u8]) -> Option<&'static str> {
    // PDF readers accept the header within the first 1024 bytes.
    let window = &head[..head.len().min(1024)];
    window.windows(5).any(|w| w == b"%PDF-").then_some(PDF_MIME)
}

/// Extracts page count and a first-page thumbnail. Both are best effort:
/// hayro does not (yet) read every PDF, e.g. encrypted ones, and a file the
/// browser's pdf.js can display must not be rejected because of that.
pub fn process_pdf(data: Vec<u8>) -> Processed {
    let mut out = Processed { mime: PDF_MIME, page_count: None, thumbnail_png: None };
    let Ok(pdf) = Pdf::new(data) else {
        tracing::warn!("hayro could not parse an uploaded PDF; storing it without page count");
        return out;
    };
    let pages = pdf.pages();
    out.page_count = Some(pages.len() as i64);
    if let Some(first) = pages.first() {
        let (width, _) = first.render_dimensions();
        if width > 0.0 {
            let scale = THUMBNAIL_WIDTH / width;
            let settings = PixmapSettings { x_scale: scale, y_scale: scale, bg_color: WHITE };
            let pixmap = render(
                first,
                &RenderCache::new(),
                &InterpreterSettings::default(),
                &RenderSettings::default(),
                &settings,
            );
            out.thumbnail_png = pixmap.into_png().ok();
        }
    }
    out
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A minimal valid PDF with `pages` empty A4 pages, for tests.
    pub fn tiny_pdf(pages: usize) -> Vec<u8> {
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            format!(
                "<< /Type /Pages /Kids [{}] /Count {pages} >>",
                (0..pages).map(|i| format!("{} 0 R", i + 3)).collect::<Vec<_>>().join(" ")
            ),
        ];
        for _ in 0..pages {
            objects.push("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] >>".to_owned());
        }
        let mut pdf = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, obj) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend(format!("{} 0 obj\n{obj}\nendobj\n", i + 1).bytes());
        }
        let xref = pdf.len();
        pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for off in offsets {
            pdf.extend(format!("{off:010} 00000 n \n").bytes());
        }
        pdf.extend(
            format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).bytes(),
        );
        pdf
    }

    #[test]
    fn reads_page_count_and_renders_thumbnail() {
        let p = process_pdf(tiny_pdf(3));
        assert_eq!(p.page_count, Some(3));
        let png = p.thumbnail_png.unwrap();
        assert_eq!(&png[1..4], b"PNG");
    }

    #[test]
    fn sniffs_pdf_only() {
        assert_eq!(sniff_mime(b"%PDF-1.4 ..."), Some(PDF_MIME));
        assert_eq!(sniff_mime(b"<?xml version"), None);
    }

    #[test]
    fn unreadable_pdf_is_kept_without_metadata() {
        let p = process_pdf(b"%PDF-1.4 garbage".to_vec());
        assert_eq!(p.page_count, None);
    }
}
