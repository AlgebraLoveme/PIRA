//! Preflight the allocation boundaries of the pinned resvg renderer.
use crate::{GuardError, MAX_PIXELS};
use resvg::{tiny_skia::Transform, usvg};

fn surface(width: f64, height: f64, kind: &str) -> Result<(), GuardError> {
    if !width.is_finite()
        || !height.is_finite()
        || width < 0.0
        || height < 0.0
        || width > i32::MAX as f64
        || height > i32::MAX as f64
        || width * height > MAX_PIXELS as f64
    {
        return Err(GuardError(format!(
            "{kind} exceeds the intermediate surface pixel limit ({MAX_PIXELS})"
        )));
    }
    Ok(())
}

// Read dimensions only, never invoke a pixel decoder here. Check the frame as
// well as the canvas for container formats; malformed small images retain the
// renderer's existing omission behavior except unsafe WebP chunk lengths.
pub(super) fn raster(data: &[u8]) -> Result<(), GuardError> {
    fn le16(b: &[u8]) -> u32 {
        u16::from_le_bytes([b[0], b[1]]) as u32
    }
    fn le24(b: &[u8]) -> u32 {
        u32::from_le_bytes([b[0], b[1], b[2], 0])
    }
    fn check(w: u32, h: u32) -> Result<(), GuardError> {
        surface(w as f64, h as f64, "embedded raster")
    }
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        if let Some(header) = data.get(12..24)
            && &header[..4] == b"IHDR"
        {
            check(
                u32::from_be_bytes(header[4..8].try_into().unwrap()),
                u32::from_be_bytes(header[8..12].try_into().unwrap()),
            )?;
        }
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        if data.len() < 13 {
            return Ok(());
        }
        check(le16(&data[6..8]), le16(&data[8..10]))?;
        let mut i = 13
            + if data[10] & 0x80 != 0 {
                3 * (1 << ((data[10] & 7) + 1))
            } else {
                0
            };
        while let Some(&tag) = data.get(i) {
            i += 1;
            match tag {
                0x2c => {
                    if let Some(frame) = data.get(i..i + 9) {
                        check(le16(&frame[4..6]), le16(&frame[6..8]))?;
                    }
                    break; // resvg decodes only the first frame.
                }
                0x21 => {
                    i += 1; // extension label
                    while let Some(&len) = data.get(i) {
                        i += 1;
                        if len == 0 {
                            break;
                        }
                        i += len as usize;
                    }
                }
                _ => break,
            }
        }
    } else if data.starts_with(b"\xff\xd8") {
        let mut i = 2;
        while data.get(i) == Some(&0xff) {
            while data.get(i) == Some(&0xff) {
                i += 1;
            }
            let Some(&marker) = data.get(i) else { break };
            i += 1;
            if matches!(marker, 0xd8 | 0x01 | 0xd0..=0xd7) {
                continue;
            }
            if matches!(marker, 0xda | 0xd9) {
                break;
            }
            let Some(length) = data.get(i..i + 2) else {
                break;
            };
            let length = u16::from_be_bytes(length.try_into().unwrap()) as usize;
            if length < 2 {
                break;
            }
            if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf)
                && let Some(frame) = data.get(i + 2..i + 7)
            {
                check(
                    u16::from_be_bytes([frame[3], frame[4]]) as u32,
                    u16::from_be_bytes([frame[1], frame[2]]) as u32,
                )?;
            }
            i += length;
        }
    } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        // ANMF embeds ordinary chunks; scanning them as a bounded worklist also
        // checks their bitstream dimensions, not just the advertised canvas.
        let mut pending = vec![&data[12..]];
        while let Some(mut chunks) = pending.pop() {
            while chunks.len() >= 8 {
                let length = u32::from_le_bytes(chunks[4..8].try_into().unwrap()) as usize;
                let body = &chunks[8..];
                if length > body.len() {
                    return Err(GuardError(
                        "invalid embedded raster WebP chunk length".into(),
                    ));
                }
                // The decoder reads fixed headers even for undersized chunk
                // declarations, so inspect available header bytes, not just length.
                match &chunks[..4] {
                    b"VP8X" if body.len() >= 10 => {
                        check(1 + le24(&body[4..7]), 1 + le24(&body[7..10]))?
                    }
                    b"VP8L" if body.len() >= 5 && body[0] == 0x2f => {
                        let bits = u32::from_le_bytes(body[1..5].try_into().unwrap());
                        check((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1)?;
                    }
                    b"VP8 " if body.len() >= 10 && body[3..6] == [0x9d, 0x01, 0x2a] => {
                        check(le16(&body[6..8]) & 0x3fff, le16(&body[8..10]) & 0x3fff)?
                    }
                    b"ANMF" if length >= 16 => {
                        check(1 + le24(&body[6..9]), 1 + le24(&body[9..12]))?;
                        pending.push(&body[16..length]);
                    }
                    _ => {}
                }
                let Some(rest) = chunks
                    .get(8..)
                    .and_then(|b| b.get(length.saturating_add(length & 1)..))
                else {
                    break;
                };
                chunks = rest;
            }
        }
    }
    Ok(())
}

pub(super) fn tree(tree: &usvg::Tree, scale: f32, canvas: (u32, u32)) -> Result<(), GuardError> {
    // Track both the inherited renderer clipping context and the current buffer:
    // embedded SVG resets the former from the latter, including inside tiles.
    let transform = Transform::from_scale(scale, scale);
    let size = (canvas.0 as f64, canvas.1 as f64);
    let extent = (size.0 * 5.0, size.1 * 5.0);
    let mut pending: Vec<_> = tree
        .root()
        .children()
        .iter()
        .map(|node| (node, transform, extent, size, 1))
        .collect();
    while let Some((node, transform, extent, size, depth)) = pending.pop() {
        if depth > crate::xml::MAX_RENDER_DEPTH * 4 {
            return Err(GuardError(
                "resolved SVG exceeds the render traversal limit".into(),
            ));
        }
        let group = match node {
            usvg::Node::Group(group) => Some(group.as_ref()),
            usvg::Node::Text(text) => Some(text.flattened()),
            _ => None,
        };
        if let Some(group) = group {
            let ts = transform.pre_concat(group.transform());
            let mut size = size;
            if group.should_isolate() {
                if let Some(bbox) = group.layer_bounding_box().transform(ts) {
                    let pad = if group.filters().is_empty() { 4.0 } else { 0.0 };
                    // PIRA: bound the clipped layer extent conservatively;
                    // exact off-canvas acceptance would require reproducing the
                    // renderer's integer clipping/translation at this boundary.
                    size = (
                        ((bbox.width() as f64).ceil().max(1.0) + pad).min(extent.0),
                        ((bbox.height() as f64).ceil().max(1.0) + pad).min(extent.1),
                    );
                    surface(size.0, size.1, "group layer")?;
                }
            }
            pending.extend(
                group
                    .children()
                    .iter()
                    .map(|n| (n, ts, extent, size, depth + 1)),
            );
            let mut clip = group.clip_path();
            while let Some(c) = clip {
                let ts = ts.pre_concat(c.transform());
                pending.extend(
                    c.root()
                        .children()
                        .iter()
                        .map(|n| (n, ts, extent, size, depth + 1)),
                );
                clip = c.clip_path();
            }
            let mut mask = group.mask();
            while let Some(m) = mask {
                pending.extend(
                    m.root()
                        .children()
                        .iter()
                        .map(|n| (n, ts, extent, size, depth + 1)),
                );
                mask = m.mask();
            }
            for filter in group.filters() {
                for primitive in filter.primitives() {
                    if let usvg::filter::Kind::Image(image) = primitive.kind() {
                        let (sx, sy) = ts.get_scale();
                        let ts = Transform::from_scale(sx, sy);
                        pending.extend(
                            image
                                .root()
                                .children()
                                .iter()
                                .map(|n| (n, ts, size, size, depth + 1)),
                        );
                    }
                }
            }
        } else if let usvg::Node::Path(path) = node {
            for paint in [
                path.fill().map(|f| f.paint()),
                path.stroke().map(|s| s.paint()),
            ]
            .into_iter()
            .flatten()
            {
                if let usvg::Paint::Pattern(pattern) = paint {
                    let (sx, sy) = transform.pre_concat(pattern.transform()).get_scale();
                    let w = (pattern.rect().width() * sx).round() as f64;
                    let h = (pattern.rect().height() * sy).round() as f64;
                    surface(w, h, "pattern tile")?;
                    let ts = Transform::from_scale(sx, sy);
                    pending.extend(
                        pattern
                            .root()
                            .children()
                            .iter()
                            .map(|n| (n, ts, extent, (w, h), depth + 1)),
                    );
                }
            }
        } else if let usvg::Node::Image(image) = node {
            if let usvg::ImageKind::SVG(svg) = image.kind() {
                let extent = (size.0 * 5.0, size.1 * 5.0);
                pending.extend(
                    svg.root()
                        .children()
                        .iter()
                        .map(|n| (n, transform, extent, size, depth + 1)),
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raster_dimensions_are_bounded_before_any_pixel_decode() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend(100_000_u32.to_be_bytes());
        png.extend(100_000_u32.to_be_bytes());
        let mut gif = b"GIF89a\x01\0\x01\0\0\0\0\x2c\0\0\0\0".to_vec();
        gif.extend(65535_u16.to_le_bytes());
        gif.extend(65535_u16.to_le_bytes());
        gif.push(0);
        let jpeg = b"\xff\xd8\xff\xe0\0\x04xx\xff\xc0\0\x0b\x08\xff\xff\xff\xff\x01\x01\x11\0";
        let mut webp = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0".to_vec();
        webp.extend([255, 255, 255, 255, 255, 255]);
        for data in [&png[..], &gif[..], &jpeg[..], &webp[..]] {
            assert!(
                raster(data)
                    .unwrap_err()
                    .to_string()
                    .contains("embedded raster")
            );
        }
        for length in [0_u32, 1, u32::MAX] {
            webp[16..20].copy_from_slice(&length.to_le_bytes());
            assert!(raster(&webp).is_err());
        }
        assert!(surface(4000.0, 4000.0, "boundary").is_ok());
        for (w, h) in [
            (4001.0, 4000.0),
            (f64::INFINITY, 1.0),
            (f64::NAN, 1.0),
            (u32::MAX as f64, 1.0),
        ] {
            assert!(surface(w, h, "boundary").is_err());
        }
        png[16..24].copy_from_slice(&[0, 0, 0, 1, 0, 0, 0, 1]);
        assert!(raster(&png).is_ok());
        assert!(raster(b"not an image").is_ok());
    }

    #[test]
    fn transformed_pattern_tiles_are_rejected_before_rendering() {
        let options = usvg::Options::default();
        for (size, pattern_transform, scene_transform, scale) in [
            (100000, "", "", 1.0),
            (100, "scale(1000)", "", 1.0),
            (100, "", "scale(1000)", 1.0),
            (100, "matrix(1 1000 1000 1 0 0)", "", 1.0),
            (100, "", "", 1000.0),
        ] {
            let source = format!(
                "<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'><defs><pattern id='p' patternUnits='userSpaceOnUse' width='{size}' height='{size}' patternTransform='{pattern_transform}'><rect width='1' height='1'/></pattern></defs><g transform='{scene_transform}'><rect width='10' height='10' fill='url(#p)'/></g></svg>"
            );
            let parsed = usvg::Tree::from_str(&source, &options).unwrap();
            assert!(
                tree(&parsed, scale, (10, 10))
                    .unwrap_err()
                    .to_string()
                    .contains("pattern tile")
            );
            // Deliberately do not render a rejected tree.
        }
        for wrapper in [
            "<g opacity='.5'>PAINT</g>",
            "<mask id='m'>PAINT</mask><rect width='10' height='10' mask='url(#m)'/>",
            "<clipPath id='c'>PAINT</clipPath><rect width='10' height='10' clip-path='url(#c)'/>",
            "<filter id='f'><feImage href='#art'/></filter><g id='art'>PAINT</g><rect width='10' height='10' filter='url(#f)'/>",
        ] {
            let body = wrapper.replace("PAINT", "<rect width='10' height='10' fill='url(#p)'/>");
            let source = format!(
                "<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'><defs><pattern id='p' patternUnits='userSpaceOnUse' width='100000' height='100000'><rect width='1' height='1'/></pattern></defs>{body}</svg>"
            );
            let parsed = usvg::Tree::from_str(&source, &options).unwrap();
            // Clip-path paint is replaced with black by usvg, so only surfaces
            // actually retained by conversion need allocation checks.
            if wrapper.starts_with("<clipPath") {
                tree(&parsed, 1.0, (10, 10)).unwrap();
            } else {
                assert!(
                    tree(&parsed, 1.0, (10, 10))
                        .unwrap_err()
                        .to_string()
                        .contains("pattern tile")
                );
            }
        }
        let embedded = "<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'><g opacity='.5'><rect width='100000' height='100000'/></g></svg>";
        let encoded: String = embedded.bytes().map(|b| format!("%{b:02X}")).collect();
        let source = format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'><defs><pattern id='p' patternUnits='userSpaceOnUse' width='1000' height='1000'><image width='10' height='10' href='data:image/svg+xml,{encoded}'/></pattern></defs><rect width='10' height='10' fill='url(#p)'/></svg>"
        );
        let parsed = usvg::Tree::from_str(&source, &options).unwrap();
        assert!(
            tree(&parsed, 1.0, (10, 10))
                .unwrap_err()
                .to_string()
                .contains("group layer")
        );
        let ordinary = "<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'><defs><pattern id='p' patternUnits='userSpaceOnUse' width='2' height='2'><rect width='1' height='1'/></pattern></defs><rect width='10' height='10' fill='url(#p)'/></svg>";
        let parsed = usvg::Tree::from_str(ordinary, &options).unwrap();
        tree(&parsed, 1.0, (10, 10)).unwrap();
        let mut image = resvg::tiny_skia::Pixmap::new(10, 10).unwrap();
        resvg::render(&parsed, Transform::identity(), &mut image.as_mut());
        assert_eq!(image.pixel(0, 0).unwrap().alpha(), 255);
        assert_eq!(image.pixel(1, 1).unwrap().alpha(), 0);
    }
}
