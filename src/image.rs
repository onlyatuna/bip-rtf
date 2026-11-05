//! List the pictures in an RTF and replace the data of one PNG picture, byte-level.
//!
//! Only the chosen `\pict` group changes. Other pictures (including a WMF stored for the same
//! drawing), fields, tags and fonts are left as they are. The new PNG is written the way WordCraft's
//! RTF export writes one: `\pngblip`, pixel size in `\picw/\pich`, display size in `\picwgoal/\pichgoal`,
//! hex data. The old `\bliptag` and `{\*\blipuid ...}` identify the old bytes, so they are dropped.

const BS: u8 = 92; // backslash

pub struct Tok {
    pub word: String,
    pub s: usize,
    pub e: usize,
    /// byte range of the numeric parameter, and its value
    pub num: Option<(usize, usize, i64)>,
}

pub struct Pict {
    pub start: usize,
    pub end: usize,
    pub data_start: usize,
    pub data_end: usize,
    pub kind: String,
    pub w: i64,
    pub h: i64,
    pub wg: i64,
    pub hg: i64,
    pub crop: bool,
    pub shape_fill: bool,
    pub toks: Vec<Tok>,
    pub uid: Option<(usize, usize)>,
}

fn group_end(b: &[u8], s: usize) -> Option<usize> {
    let mut d = 0usize;
    let mut i = s;
    while i < b.len() {
        match b[i] {
            BS => i += 1,
            b'{' => d += 1,
            b'}' => {
                d = d.checked_sub(1)?;
                if d == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

const KINDS: &[&str] = &["pngblip", "jpegblip", "emfblip", "wmetafile", "dibitmap", "wbitmap", "macpict", "pmmetafile"];

pub fn scan(b: &[u8]) -> Vec<Pict> {
    let mut out = Vec::new();
    let pat = [BS, b'p', b'i', b'c', b't'];
    let mut from = 0;
    while let Some(off) = b[from..].windows(pat.len()).position(|w| w == pat) {
        let at = from + off;
        from = at + pat.len();
        if b.get(from).is_some_and(|c| c.is_ascii_alphabetic()) {
            continue; // \pictXYZ
        }
        let Some(start) = b[..at].iter().rposition(|&c| c == b'{') else { continue };
        if !b[start + 1..at].iter().all(|c| c.is_ascii_whitespace()) {
            continue;
        }
        let Some(end) = group_end(b, start) else { continue };

        let mut toks = Vec::new();
        let mut uid = None;
        let mut i = from;
        loop {
            while i < end && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if i >= end {
                break;
            }
            if b[i] == BS {
                let s = i;
                let mut j = i + 1;
                while j < end && b[j].is_ascii_alphabetic() {
                    j += 1;
                }
                let word = String::from_utf8_lossy(&b[i + 1..j]).to_string();
                let ns = j;
                if j < end && (b[j] == b'-' || b[j].is_ascii_digit()) {
                    j += 1;
                    while j < end && b[j].is_ascii_digit() {
                        j += 1;
                    }
                }
                let num = (j > ns).then(|| (ns, j, std::str::from_utf8(&b[ns..j]).ok().and_then(|x| x.parse().ok()).unwrap_or(0)));
                if j < end && b[j] == b' ' {
                    j += 1;
                }
                toks.push(Tok { word, s, e: j, num });
                i = j;
            } else if b[i] == b'{' {
                let Some(ge) = group_end(b, i) else { break };
                if b[i..ge].starts_with(&[b'{', BS, b'*', BS]) && b[i..ge].windows(8).any(|w| w == b"blipuid ") {
                    uid = Some((i, ge));
                }
                i = ge;
            } else {
                break;
            }
        }
        let val = |w: &str| toks.iter().find(|t| t.word == w).and_then(|t| t.num).map_or(0, |n| n.2);
        let kind = toks.iter().find(|t| KINDS.contains(&t.word.as_str())).map_or("unknown", |t| t.word.as_str()).to_string();
        let crop = ["piccropl", "piccropr", "piccropt", "piccropb"].iter().any(|w| val(w) != 0);
        let look = &b[start.saturating_sub(40)..start];
        let shape_fill = look.windows(8).any(|w| w == b"sn pib}{" || w == b"\\sn pib}");
        out.push(Pict {
            start,
            end,
            data_start: i,
            data_end: end - 1,
            kind,
            w: val("picw"),
            h: val("pich"),
            wg: val("picwgoal"),
            hg: val("pichgoal"),
            crop,
            shape_fill,
            toks,
            uid,
        });
    }
    out
}

fn hex_decode(s: &[u8]) -> Vec<u8> {
    let d: Vec<u8> = s.iter().copied().filter(|c| c.is_ascii_hexdigit()).collect();
    d.chunks(2).filter(|p| p.len() == 2).filter_map(|p| u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok()).collect()
}

/// PNG bytes of a `\pngblip` picture.
pub fn png_of(b: &[u8], p: &Pict) -> Option<Vec<u8>> {
    (p.kind == "pngblip").then(|| hex_decode(&b[p.data_start..p.data_end]))
}

pub fn png_size(d: &[u8]) -> Result<(u32, u32), String> {
    if d.len() < 33 || d[..8] != [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a] || &d[12..16] != b"IHDR" {
        return Err("not a PNG file".into());
    }
    let w = u32::from_be_bytes([d[16], d[17], d[18], d[19]]);
    let h = u32::from_be_bytes([d[20], d[21], d[22], d[23]]);
    if w == 0 || h == 0 {
        return Err("PNG has zero width or height".into());
    }
    Ok((w, h))
}

pub struct Replaced {
    pub bytes: Vec<u8>,
    pub old_px: (i64, i64),
    pub new_px: (u32, u32),
    pub old_goal: (i64, i64),
    pub new_goal: (i64, i64),
    pub warning: Option<String>,
}

/// Replace picture number `idx` (0-based) with `png`.
pub fn replace_png(b: &[u8], idx: usize, png: &[u8], stretch: bool) -> Result<Replaced, String> {
    let pics = scan(b);
    let p = pics.get(idx).ok_or_else(|| format!("no picture {} (the file has {})", idx + 1, pics.len()))?;
    if p.kind != "pngblip" {
        return Err(format!("picture {} is {}, not a PNG; only PNG pictures can be replaced", idx + 1, p.kind));
    }
    if p.crop {
        return Err("this picture is cropped; the crop would apply to the new image. Remove the crop in Word first.".into());
    }
    let (nw, nh) = png_size(png)?;
    if png.len() > 20 * 1024 * 1024 {
        return Err("PNG is larger than 20 MB".into());
    }
    let (wg, hg) = if stretch || p.wg <= 0 || p.hg <= 0 {
        (p.wg.max(1), p.hg.max(1))
    } else {
        let scale = (p.wg as f64 / f64::from(nw)).min(p.hg as f64 / f64::from(nh));
        (((f64::from(nw) * scale).round() as i64).max(1), ((f64::from(nh) * scale).round() as i64).max(1))
    };
    let mut warning = None;
    if p.shape_fill && p.hg > 0 && p.wg > 0 {
        let (a, c) = (p.wg as f64 / p.hg as f64, f64::from(nw) / f64::from(nh));
        if (a / c - 1.0).abs() > 0.02 {
            warning = Some(format!(
                "this picture fills a drawing shape: the shape's frame decides the displayed size, so a new image with a different shape ({nw}x{nh}, ratio {c:.2} vs {a:.2}) will look stretched"
            ));
        }
    }

    // Header edits, applied from the end.
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let set = |name: &str, v: i64, edits: &mut Vec<(usize, usize, String)>| -> Result<(), String> {
        let t = p.toks.iter().find(|t| t.word == name).and_then(|t| t.num).ok_or_else(|| format!("picture has no \\{name}"))?;
        edits.push((t.0, t.1, v.to_string()));
        Ok(())
    };
    set("picw", i64::from(nw), &mut edits)?;
    set("pich", i64::from(nh), &mut edits)?;
    set("picwgoal", wg, &mut edits)?;
    set("pichgoal", hg, &mut edits)?;
    for t in p.toks.iter().filter(|t| t.word == "bliptag") {
        edits.push((t.s, t.e, String::new()));
    }
    if let Some((s, e)) = p.uid {
        edits.push((s, e, String::new()));
    }
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    let mut header = b[p.start..p.data_start].to_vec();
    for (s, e, with) in edits {
        header.splice(s - p.start..e - p.start, with.into_bytes());
    }
    while header.last().is_some_and(|c| c.is_ascii_whitespace()) {
        header.pop();
    }
    header.push(b'\n');

    let mut out = Vec::with_capacity(b.len() + png.len() * 2);
    out.extend_from_slice(&b[..p.start]);
    out.extend_from_slice(&header);
    for chunk in png.chunks(64) {
        for x in chunk {
            out.extend_from_slice(format!("{x:02x}").as_bytes());
        }
        out.push(b'\n');
    }
    out.extend_from_slice(&b[p.data_end..]);
    Ok(Replaced { bytes: out, old_px: (p.w, p.h), new_px: (nw, nh), old_goal: (p.wg, p.hg), new_goal: (wg, hg), warning })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_png(w: u32, h: u32) -> Vec<u8> {
        let mut v = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, b'I', b'H', b'D', b'R'];
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        v
    }

    fn doc(png: &[u8]) -> Vec<u8> {
        let hex: String = png.iter().map(|b| format!("{b:02x}")).collect();
        let t = format!(
            "{{~rtf1 a {{~pict~picscalex100~picw40~pich20~picwgoal2000~pichgoal1000~pngblip~bliptag77{{~*~blipuid abc}}{hex}}} b {{~pict~wmetafile8~picw1~pich1~picwgoal5~pichgoal5 0102}}}}"
        );
        t.replace('~', &(BS as char).to_string()).into_bytes()
    }

    #[test]
    fn scan_finds_both_pictures() {
        let b = doc(&tiny_png(40, 20));
        let p = scan(&b);
        assert_eq!(p.len(), 2);
        assert_eq!((p[0].kind.as_str(), p[0].w, p[0].h, p[0].wg, p[0].hg), ("pngblip", 40, 20, 2000, 1000));
        assert_eq!(p[1].kind, "wmetafile");
        assert_eq!(png_of(&b, &p[0]).unwrap(), tiny_png(40, 20));
        assert!(p[0].uid.is_some());
    }

    #[test]
    fn replace_keeps_aspect_inside_old_box_and_leaves_the_wmf() {
        let b = doc(&tiny_png(40, 20));
        let new = tiny_png(100, 100);
        let r = replace_png(&b, 0, &new, false).unwrap();
        let p = scan(&r.bytes);
        assert_eq!(p.len(), 2);
        assert_eq!((p[0].w, p[0].h), (100, 100));
        assert_eq!((p[0].wg, p[0].hg), (1000, 1000)); // fits the 2000x1000 box
        assert_eq!(png_of(&r.bytes, &p[0]).unwrap(), new);
        assert!(p[0].uid.is_none() && !p[0].toks.iter().any(|t| t.word == "bliptag"));
        assert_eq!(p[1].kind, "wmetafile");
        let s = replace_png(&b, 0, &new, true).unwrap();
        assert_eq!((s.new_goal.0, s.new_goal.1), (2000, 1000));
    }

    #[test]
    fn rejects_non_png_and_wmf_target() {
        let b = doc(&tiny_png(40, 20));
        assert!(replace_png(&b, 0, b"not a png at all, not a png at all, not", false).is_err());
        assert!(replace_png(&b, 1, &tiny_png(1, 1), false).is_err());
        assert!(replace_png(&b, 5, &tiny_png(1, 1), false).is_err());
    }
}
