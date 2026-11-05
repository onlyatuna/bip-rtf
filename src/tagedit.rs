//! Rename one template tag, byte-level, leaving everything else in the RTF unchanged.
//!
//! A tag lives in up to four places:
//!   1. the Base64 `\docvar` that holds the real tag text (what the BI Publisher engine reads),
//!   2. for a simple field `<?NAME?>`, the Word form field's default text (`\ffdeftext NAME`),
//!   3. the same label inside the binary `\datafield` (UTF-16, with a length),
//!   4. the visible result text (`\fldrslt`).
//! Tags typed as plain text are replaced where they sit in one piece.

const BS: u8 = 92; // backslash

#[derive(Default, Debug)]
pub struct Report {
    pub docvars: usize,
    pub typed: usize,
    pub labels: usize,
    pub label_skipped: Vec<String>,
}

fn find(b: &[u8], pat: &[u8], from: usize) -> Option<usize> {
    if from >= b.len() || pat.is_empty() {
        return None;
    }
    b[from..].windows(pat.len()).position(|w| w == pat).map(|i| i + from)
}

fn rfind(b: &[u8], pat: &[u8], before: usize) -> Option<usize> {
    let end = before.min(b.len());
    if end < pat.len() {
        return None;
    }
    b[..end].windows(pat.len()).rposition(|w| w == pat)
}

fn replace_all(b: &[u8], from: &[u8], to: &[u8]) -> (Vec<u8>, usize) {
    let mut out = Vec::with_capacity(b.len());
    let (mut i, mut n) = (0, 0);
    while i < b.len() {
        if b[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
            n += 1;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    (out, n)
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_encode(d: &[u8]) -> String {
    let mut s = String::new();
    for c in d.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

fn pat(tail: &[u8]) -> Vec<u8> {
    let mut v = vec![b'{', BS, b'*', BS];
    v.extend_from_slice(tail);
    v
}

fn hex_decode(s: &[u8]) -> Option<Vec<u8>> {
    let d: Vec<u8> = s.iter().copied().filter(|c| c.is_ascii_hexdigit()).collect();
    if d.len() % 2 != 0 {
        return None;
    }
    d.chunks(2).map(|p| u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok()).collect()
}

fn hex_encode(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
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

pub fn is_simple(tag: &str) -> Option<&str> {
    let inner = tag.strip_prefix("<?")?.strip_suffix("?>")?;
    (!inner.is_empty() && inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')).then_some(inner)
}

fn check_tag(t: &str, what: &str) -> Result<(), String> {
    if !(t.starts_with("<?") && t.ends_with("?>") && t.len() > 4) {
        return Err(format!("{what} must be a whole tag like <?NAME?>"));
    }
    if t.chars().any(|c| c == char::from(BS) || c == '{' || c == '}' || c.is_control()) {
        return Err(format!("{what} must not contain backslash, braces or control characters"));
    }
    if t.starts_with("<?ref:") {
        return Err(format!("{what} must not be a <?ref:...?> pointer"));
    }
    Ok(())
}

/// Visible text spans of a group: runs of literal bytes (not control words, not braces).
fn literal_spans(b: &[u8], s: usize, e: usize) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut i = s;
    let mut cur: Option<usize> = None;
    let close = |cur: &mut Option<usize>, i: usize, spans: &mut Vec<(usize, usize)>| {
        if let Some(st) = cur.take() {
            spans.push((st, i));
        }
    };
    while i < e {
        match b[i] {
            BS => {
                close(&mut cur, i, &mut spans);
                if b.get(i + 1) == Some(&b'\'') {
                    i += 4;
                    continue;
                }
                let mut j = i + 1;
                while j < e && b[j].is_ascii_alphabetic() {
                    j += 1;
                }
                if j < e && (b[j] == b'-' || b[j].is_ascii_digit()) {
                    j += 1;
                    while j < e && b[j].is_ascii_digit() {
                        j += 1;
                    }
                }
                if j < e && b[j] == b' ' {
                    j += 1;
                }
                i = j.max(i + 2);
            }
            b'{' | b'}' | b'\r' | b'\n' => {
                close(&mut cur, i, &mut spans);
                i += 1;
            }
            _ => {
                cur.get_or_insert(i);
                i += 1;
            }
        }
    }
    close(&mut cur, i, &mut spans);
    spans
}

struct Edit {
    start: usize,
    end: usize,
    with: Vec<u8>,
}

/// Label edits for the form field that points at `ref_name`; `None` plus a reason when it cannot be done safely.
fn label_edits(b: &[u8], ref_name: &str, old: &str, new: &str) -> Result<Vec<Edit>, String> {
    let ref_tag = format!("<?ref:{ref_name}?>");
    let p = find(b, ref_tag.as_bytes(), 0).ok_or("no field points at this docvar")?;
    let mut edits = Vec::new();

    // \ffdeftext
    let mark = pat(b"ffdeftext ");
    let d = rfind(b, &mark, p).filter(|d| p - d < 600).ok_or("no ffdeftext")?;
    let (ds, de) = (d + mark.len(), find(b, b"}", d).ok_or("bad ffdeftext")?);
    if &b[ds..de] != old.as_bytes() {
        return Err("form field default text differs from the tag name".into());
    }
    edits.push(Edit { start: ds, end: de, with: new.as_bytes().to_vec() });

    // \datafield (binary, hex): count (u16 le) + UTF-16LE label
    let dm = pat(b"datafield ");
    let f = rfind(b, &dm, d).filter(|f| d - f < 1200).ok_or("no datafield")?;
    let (hs, he) = (f + dm.len(), find(b, b"}", f).ok_or("bad datafield")?);
    let mut bin = hex_decode(&b[hs..he]).ok_or("datafield is not hex")?;
    let enc = |s: &str| -> Vec<u8> {
        let u: Vec<u16> = s.encode_utf16().collect();
        let mut v = (u.len() as u16).to_le_bytes().to_vec();
        v.extend(u.iter().flat_map(|c| c.to_le_bytes()));
        v
    };
    let (oldb, newb) = (enc(old), enc(new));
    let hits: Vec<usize> = bin.windows(oldb.len()).enumerate().filter(|(_, w)| *w == oldb.as_slice()).map(|(i, _)| i).collect();
    if hits.len() != 1 {
        return Err(format!("expected the label once in the datafield, found {}", hits.len()));
    }
    bin.splice(hits[0]..hits[0] + oldb.len(), newb);
    edits.push(Edit { start: hs, end: he, with: hex_encode(&bin).into_bytes() });

    // \fldrslt visible text
    let r = find(b, &[&[b'{', BS][..], &b"fldrslt"[..]].concat(), p).filter(|r| r - p < 4000).ok_or("no fldrslt")?;
    let re = group_end(b, r).ok_or("bad fldrslt")?;
    let hit: Vec<(usize, usize)> = literal_spans(b, r + 1, re)
        .into_iter()
        .filter_map(|(s, e)| {
            let t = &b[s..e];
            let lead = t.iter().take_while(|c| **c == b' ').count();
            let trimmed = t[lead..].trim_ascii_end();
            (trimmed == old.as_bytes()).then_some((s + lead, s + lead + trimmed.len()))
        })
        .collect();
    if hit.len() != 1 {
        return Err("visible text is not a single run equal to the old label".into());
    }
    edits.push(Edit { start: hit[0].0, end: hit[0].1, with: new.as_bytes().to_vec() });
    Ok(edits)
}

pub fn set_tag(b: &[u8], from: &str, to: &str) -> Result<(Vec<u8>, Report), String> {
    check_tag(from, "--from")?;
    check_tag(to, "--to")?;
    if from == to {
        return Err("--from and --to are the same".into());
    }
    let mut rep = Report::default();

    // 1. docvars
    let marker = pat(b"docvar ");
    let mut out = Vec::with_capacity(b.len() + 64);
    let mut i = 0;
    let mut whole: Vec<String> = Vec::new(); // docvars whose whole text was the tag
    while let Some(m) = find(b, &marker, i) {
        let ge = group_end(b, m).ok_or("unterminated docvar")?;
        let body = &b[m + marker.len()..ge - 1];
        let n1 = find(body, b"}", 0);
        let v0 = n1.and_then(|n| find(body, b"{", n));
        let v1 = v0.and_then(|v| find(body, b"}", v));
        if let (Some(n1), Some(v0), Some(v1)) = (n1, v0, v1) {
            let name = std::str::from_utf8(&body[1..n1]).unwrap_or("").to_string();
            let value = std::str::from_utf8(&body[v0 + 1..v1]).unwrap_or("");
            let raw = crate::rtf::b64_decode(value);
            if let Ok(text) = String::from_utf8(raw)
                && text.contains(from)
            {
                if text.trim() == from {
                    whole.push(name.clone());
                }
                let new_text = text.replace(from, to);
                let base = m + marker.len() + v0 + 1;
                out.extend_from_slice(&b[i..base]);
                out.extend_from_slice(b64_encode(new_text.as_bytes()).as_bytes());
                i = m + marker.len() + v1;
                rep.docvars += 1;
                continue;
            }
        }
        out.extend_from_slice(&b[i..ge]);
        i = ge;
    }
    out.extend_from_slice(&b[i..]);

    // 2. typed text, outside docvar values (those are Base64 and cannot hold `<?`)
    // Non-ASCII typed text is stored as escapes in the RTF and is not matched here; the caller's
    // tag-list check then refuses the edit if such a typed tag exists.
    if from.is_ascii() && to.is_ascii() {
        let (out2, n) = replace_all(&out, from.as_bytes(), to.as_bytes());
        out = out2;
        rep.typed = n;
    }
    if rep.docvars == 0 && rep.typed == 0 {
        return Err(format!("tag {from} not found as a docvar or as typed text"));
    }

    // 3. labels of simple form fields
    if let (Some(old), Some(new)) = (is_simple(from), is_simple(to)) {
        for name in whole {
            match label_edits(&out, &name, old, new) {
                Ok(mut edits) => {
                    edits.sort_by_key(|e| std::cmp::Reverse(e.start));
                    for e in edits {
                        out.splice(e.start..e.end, e.with);
                    }
                    rep.labels += 1;
                }
                Err(why) => rep.label_skipped.push(format!("{name}: {why}")),
            }
        }
    } else if !whole.is_empty() {
        rep.label_skipped.push("tag is not a simple <?NAME?>: the field's visible label was left as it is".into());
    }
    Ok((out, rep))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trip() {
        for s in ["", "a", "ab", "abc", "<?FIELD_ONE?>", "<?x?>\r\n<fo:y a=\"{//H/I}\"?>"] {
            assert_eq!(crate::rtf::b64_decode(&b64_encode(s.as_bytes())), s.as_bytes(), "{s}");
        }
        assert_eq!(b64_encode(b"<?FIELD_ONE?>"), "PD9GSUVMRF9PTkU/Pg==");
    }

    #[test]
    fn simple_tag_detection() {
        assert_eq!(is_simple("<?CUSTOMER?>"), Some("CUSTOMER"));
        assert_eq!(is_simple("<?for-each:X?>"), None);
        assert!(check_tag("NAME", "t").is_err());
        assert!(check_tag("<?ref:xdo1?>", "t").is_err());
    }

    #[test]
    fn non_ascii_tag_in_a_docvar_is_renamed() {
        let value = b64_encode("<?姓名?>".as_bytes());
        let src = format!("{{{bs}rtf1 {{{bs}*{bs}docvar {{xdo0001}}{{{value}}}}}}}", bs = char::from(BS)).into_bytes();
        let (out, rep) = set_tag(&src, "<?姓名?>", "<?名前?>").unwrap();
        assert_eq!(rep.docvars, 1);
        let s = String::from_utf8(out).unwrap();
        let v = s.split("{xdo0001}{").nth(1).unwrap().split('}').next().unwrap();
        assert_eq!(crate::rtf::b64_decode(v), "<?名前?>".as_bytes());
    }

    #[test]
    fn typed_tag_is_replaced() {
        let src = b"{\\rtf1 x <?OLD?> y <?OLD?> z}".to_vec();
        let (out, rep) = set_tag(&src, "<?OLD?>", "<?NEW?>").unwrap();
        assert_eq!(out, b"{\\rtf1 x <?NEW?> y <?NEW?> z}".to_vec());
        assert_eq!(rep.typed, 2);
        assert!(set_tag(&src, "<?MISSING?>", "<?X?>").is_err());
    }
}
