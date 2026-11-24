#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlobKind {
    Image,
    Pdf,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobRef {
    pub hash: String,
    pub name: String,
    pub kind: BlobKind,
    pub range: std::ops::Range<usize>,
}

pub fn is_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

pub fn blob_refs(text: &str) -> Vec<BlobRef> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(off) = text[i..].find("blob:") {
        let at = i + off;
        let hash_start = at + 5;
        let hash = text.get(hash_start..hash_start + 64).unwrap_or("");
        i = hash_start;
        if !is_hash(hash) {
            continue;
        }
        let after = hash_start + 64;
        let before = text[..at].trim_end();
        if before.ends_with("{{pdf:") && text[after..].trim_start().starts_with("}}") {
            let start = before.len() - "{{pdf:".len();
            let end = after + (text[after..].len() - text[after..].trim_start().len()) + 2;
            out.push(BlobRef {
                hash: hash.to_string(),
                name: "PDF".to_string(),
                kind: BlobKind::Pdf,
                range: start..end,
            });
            i = end;
            continue;
        }
        if text[..at].ends_with("](") && text[after..].starts_with(')') {
            let close = at - 2;
            if let Some(open) = text[..close].rfind('[') {
                let image = open > 0 && text.as_bytes()[open - 1] == b'!';
                let name = text[open + 1..close].to_string();
                let kind = if image {
                    BlobKind::Image
                } else if name.to_ascii_lowercase().ends_with(".pdf") {
                    BlobKind::Pdf
                } else {
                    BlobKind::File
                };
                out.push(BlobRef {
                    hash: hash.to_string(),
                    name,
                    kind,
                    range: (if image { open - 1 } else { open })..after + 1,
                });
                i = after + 1;
            }
        }
    }
    out
}

pub fn sniff_mime(name: &str, bytes: &[u8]) -> &'static str {
    let magic: &[(&[u8], &str)] = &[
        (b"\x89PNG\r\n\x1a\n", "image/png"),
        (b"\xff\xd8\xff", "image/jpeg"),
        (b"GIF8", "image/gif"),
        (b"%PDF-", "application/pdf"),
    ];
    for (m, t) in magic {
        if bytes.starts_with(m) {
            return t;
        }
    }
    if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return "image/webp";
    }
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" | "md" => "text/plain",
        _ => "application/octet-stream",
    }
}

pub fn markup(name: &str, hash: &str, mime: &str) -> String {
    let name: String = name
        .chars()
        .map(|c| if matches!(c, '[' | ']') { '_' } else { c })
        .collect();
    if mime.starts_with("image/") {
        format!("![{name}](blob:{hash})")
    } else {
        format!("[{name}](blob:{hash})")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Highlight {
    pub hash: String,
    pub page: usize,
    pub rects: Vec<[f32; 4]>,
    pub range: std::ops::Range<usize>,
}

const MAX_HIGHLIGHT_RECTS: usize = 64;

pub fn highlight_markup(hash: &str, page: usize, rects: &[[f32; 4]]) -> String {
    let rects: Vec<String> = rects
        .iter()
        .take(MAX_HIGHLIGHT_RECTS)
        .map(|r| {
            r.iter()
                .map(|v| format!("{:.4}", v.clamp(0., 1.)))
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect();
    format!(
        "{{{{highlight: blob:{hash} {} {}}}}}",
        page + 1,
        rects.join(";")
    )
}

pub fn parse_highlight(inner: &str) -> Option<(String, usize, Vec<[f32; 4]>)> {
    let body = inner.trim().strip_prefix("highlight:")?.trim();
    let mut parts = body.split_whitespace();
    let hash = parts.next()?.strip_prefix("blob:")?;
    let page: usize = parts.next()?.parse().ok()?;
    let rects_src = parts.next()?;
    if !is_hash(hash) || page == 0 || parts.next().is_some() {
        return None;
    }
    let mut rects = Vec::new();
    for r in rects_src.split(';') {
        let nums: Vec<f32> = r
            .split(',')
            .map(|v| v.parse::<f32>())
            .collect::<Result<_, _>>()
            .ok()?;
        if nums.len() != 4 || nums.iter().any(|v| !v.is_finite() || *v < 0. || *v > 1.) {
            return None;
        }
        rects.push([nums[0], nums[1], nums[2], nums[3]]);
    }
    if rects.is_empty() || rects.len() > MAX_HIGHLIGHT_RECTS {
        return None;
    }
    Some((hash.to_string(), page - 1, rects))
}

pub fn highlights(text: &str) -> Vec<Highlight> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(off) = text[i..].find("{{highlight:") {
        let start = i + off;
        let Some(close) = text[start..].find("}}") else {
            break;
        };
        let end = start + close + 2;
        if let Some((hash, page, rects)) = parse_highlight(&text[start + 2..end - 2]) {
            out.push(Highlight {
                hash,
                page,
                rects,
                range: start..end,
            });
        }
        i = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

    #[test]
    fn finds_attachment_refs() {
        let t = format!("see ![cat.png](blob:{H}) and [notes.txt](blob:{H}) {{{{pdf: blob:{H}}}}}");
        let r = blob_refs(&t);
        assert_eq!(r.len(), 3, "{r:?}");
        assert_eq!(
            (r[0].kind, r[0].name.as_str()),
            (BlobKind::Image, "cat.png")
        );
        assert_eq!(&t[r[0].range.clone()], format!("![cat.png](blob:{H})"));
        assert_eq!(
            (r[1].kind, r[1].name.as_str()),
            (BlobKind::File, "notes.txt")
        );
        assert_eq!(r[2].kind, BlobKind::Pdf);
        assert_eq!(&t[r[2].range.clone()], format!("{{{{pdf: blob:{H}}}}}"));
        assert!(blob_refs("blob:123 [x](blob:zz)").is_empty());
        assert!(
            blob_refs(&format!("bare blob:{H}")).is_empty(),
            "markup only"
        );
    }

    #[test]
    fn highlight_markup_round_trips() {
        let m = highlight_markup(H, 2, &[[0.1, 0.25, 0.5, 0.05]]);
        assert_eq!(
            m,
            format!("{{{{highlight: blob:{H} 3 0.1000,0.2500,0.5000,0.0500}}}}")
        );
        let t = format!("\"systems thinking\" {m}");
        let h = highlights(&t);
        assert_eq!(h.len(), 1);
        assert_eq!(
            (h[0].page, h[0].rects.clone()),
            (2, vec![[0.1, 0.25, 0.5, 0.05]])
        );
        assert_eq!(&t[h[0].range.clone()], m);
        let two = [[0.1, 0.1, 0.8, 0.03], [0.1, 0.13, 0.4, 0.03]];
        let h = highlights(&highlight_markup(H, 0, &two));
        assert_eq!(h[0].rects, two.to_vec());
        for bad in [
            format!("{{{{highlight: blob:{H} 0 0,0,1,1}}}}"),
            format!("{{{{highlight: blob:{H} 1 0,0,2,1}}}}"),
            format!("{{{{highlight: blob:{H} 1 0,0,1}}}}"),
            format!("{{{{highlight: blob:{H} 1 0,0,1,1;}}}}"),
            "{{highlight: blob:xyz 1 0,0,1,1}}".to_string(),
        ] {
            assert!(highlights(&bad).is_empty(), "{bad}");
        }
    }

    #[test]
    fn markup_round_trips_and_sniffs() {
        for (name, mime, kind) in [
            ("a.png", "image/png", BlobKind::Image),
            ("doc.zip", "application/zip", BlobKind::File),
        ] {
            let m = markup(name, H, mime);
            let r = blob_refs(&m);
            assert_eq!((r.len(), r[0].kind), (1, kind), "{m}");
        }
        let pdf = blob_refs(&markup("paper [v2].pdf", H, "application/pdf"));
        assert_eq!(pdf.len(), 1);
        assert_eq!(
            (pdf[0].kind, pdf[0].name.as_str()),
            (BlobKind::Pdf, "paper _v2_.pdf")
        );
        assert_eq!(sniff_mime("x", b"%PDF-1.7"), "application/pdf");
        assert_eq!(sniff_mime("x.bin", b"\x89PNG\r\n\x1a\n...."), "image/png");
        assert_eq!(sniff_mime("photo.JPG", b"??"), "image/jpeg");
        assert_eq!(sniff_mime("x", b""), "application/octet-stream");
    }
}

#[cfg(test)]
mod inline_tests {
    use crate::inline::{parse, Inline};

    #[test]
    fn image_markup_is_one_link_atom() {
        let h = "a".repeat(64);
        let t = format!("x ![p.png](blob:{h}) y");
        let nodes = parse(&t);
        let link = nodes
            .iter()
            .find_map(|n| match n {
                Inline::Link { text, url, range } => {
                    Some((text.clone(), url.clone(), range.clone()))
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(link.0, "p.png");
        assert_eq!(
            &t[link.2.clone()],
            format!("![p.png](blob:{h})"),
            "the ! belongs to it"
        );
        assert!(matches!(&nodes[0], Inline::Text(s) if s == "x "));
        assert!(parse("![[x]]")
            .iter()
            .any(|n| matches!(n, Inline::PageRef { .. })));
    }
}
