//! QR code generation for the "二维码" (QR code) share method.
//!
//! Uses the pure-Rust `qrcode` crate to encode the share URL, then renders the
//! module matrix as a self-contained SVG string (no `image` crate needed). The
//! SVG is returned as a data URL ready to embed in a dialog `<img>` or hand to
//! `cx.open_url`.

use qrcode::QrCode;

/// Encode `text` into an SVG string. Returns `None` if encoding fails (e.g. the
/// input is too long for the QR capacity).
pub fn to_svg(text: &str) -> Option<String> {
    let code = QrCode::new(text.as_bytes()).ok()?;
    let width = code.width();
    // Each module = 10 SVG units; add a 4-module quiet zone border.
    let module = 10u32;
    let quiet = 4u32;
    let total = (width as u32 + 2 * quiet) * module;
    let mut svg = String::with_capacity(4 * 1024);
    svg.push_str(&format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{total}" height="{total}" viewBox="0 0 {total} {total}" shape-rendering="crispEdges">"##
    ));
    // White background.
    svg.push_str(&format!(
        r##"<rect width="{total}" height="{total}" fill="#ffffff"/>"##
    ));
    // Dark modules as a single <path> of 1-unit rects for compactness.
    svg.push_str(r##"<path fill="#000000" d=""##);
    for y in 0..width {
        for x in 0..width {
            if code[(x, y)] == qrcode::Color::Dark {
                let px = (x as u32 + quiet) * module;
                let py = (y as u32 + quiet) * module;
                svg.push_str(&format!("M{px},{py}h{module}v{module}h-{module}z"));
            }
        }
    }
    svg.push_str(r##""/></svg>"##);
    Some(svg)
}

/// Encode `text` into an SVG data URL (`data:image/svg+xml;base64,...`).
pub fn to_svg_data_url(text: &str) -> Option<String> {
    let svg = to_svg(text)?;
    // URL-encode the SVG for a safe data URL (no base64 needed — keeps it
    // human-debuggable and avoids the base64 encoding step).
    let encoded = svg
        .replace('#', "%23")
        .replace('<', "%3C")
        .replace('>', "%3E")
        .replace('"', "'")
        .replace('\n', "");
    Some(format!("data:image/svg+xml,{encoded}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_a_url() {
        let svg = to_svg("https://example.com/share/abc12345").unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("shape-rendering"));
        assert!(svg.contains("</svg>"));
    }

    #[test]
    fn data_url_is_well_formed() {
        let url = to_svg_data_url("hello").unwrap();
        assert!(url.starts_with("data:image/svg+xml,"));
        assert!(url.contains("%3Csvg"));
    }

    #[test]
    fn empty_input_produces_minimal_qr() {
        // qrcode 0.14 accepts empty input (encodes to a minimal QR), so we
        // expect a valid SVG, not None.
        let svg = to_svg("");
        assert!(svg.is_some(), "empty input should produce a minimal QR");
    }
}
