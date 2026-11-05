//! Byte-level edits that leave everything except the touched bytes unchanged.
//!
//! Only one edit exists so far: rename entries in the RTF font table. The body, `\docvar` table and
//! form fields are never touched, so BI Publisher tags cannot be affected by this edit.

use crate::rtf::encoding_for_charset;

const BS: u8 = 92; // backslash, written as a number to keep source free of escape sequences

fn hexval(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

/// End index (exclusive) of the group starting at `s` (`b[s] == '{'`).
fn group_end(b: &[u8], s: usize) -> Option<usize> {
    let mut d = 0usize;
    let mut i = s;
    while i < b.len() {
        match b[i] {
            BS => i += 1, // skip escaped char
            b'{' => d += 1,
            b'}' => {
                d -= 1;
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

pub struct Renamed {
    pub index: String,
    pub old: String,
}

/// Rename every font table entry whose name equals `from` (case-insensitive) to `to`.
pub fn rename_font(b: &[u8], from: &str, to: &str) -> Result<(Vec<u8>, Vec<Renamed>), String> {
    if to.chars().any(|c| matches!(c, '\\' | '{' | '}' | ';') || c.is_control()) {
        return Err("new font name must not contain \\ { } ; or control characters".into());
    }
    let key = [BS].iter().chain(b"fonttbl").copied().collect::<Vec<u8>>();
    let at = b.windows(key.len()).position(|w| w == key.as_slice()).ok_or("no font table")?;
    let tbl_start = b[..at].iter().rposition(|&c| c == b'{').ok_or("bad font table")?;
    let tbl_end = group_end(b, tbl_start).ok_or("unterminated font table")?;

    let mut out: Vec<u8> = Vec::with_capacity(b.len() + 64);
    let mut last = 0usize;
    let mut done: Vec<Renamed> = Vec::new();
    let mut i = tbl_start + 1;
    while i < tbl_end - 1 {
        if b[i] != b'{' {
            i += 1;
            continue;
        }
        let e_end = group_end(b, i).ok_or("unterminated font entry")?;
        if let Some((s, e, enc_cs, idx)) = name_span(b, i, e_end) {
            let enc = encoding_for_charset(enc_cs);
            let bytes = unescape(&b[s..e]);
            let (name, _, _) = enc.unwrap_or(encoding_rs::WINDOWS_1252).decode(&bytes);
            if name.trim().eq_ignore_ascii_case(from.trim()) || name.trim() == from.trim() {
                let mut repl: Vec<u8> = Vec::new();
                for c in to.chars() {
                    if c.is_ascii() {
                        repl.push(c as u8);
                    } else {
                        let e = enc.ok_or_else(|| format!("cannot encode {c} in this font's charset"))?;
                        let mut buf = [0u8; 4];
                        let (enc_bytes, _, bad) = e.encode(c.encode_utf8(&mut buf));
                        if bad {
                            return Err(format!("{c} cannot be written in charset {enc_cs}; use an ASCII name"));
                        }
                        for x in enc_bytes.iter() {
                            repl.extend_from_slice(format!("{}'{:02x}", BS as char, x).as_bytes());
                        }
                    }
                }
                out.extend_from_slice(&b[last..s]);
                out.extend_from_slice(&repl);
                last = e;
                done.push(Renamed { index: idx, old: name.trim().to_string() });
            }
        }
        i = e_end;
    }
    out.extend_from_slice(&b[last..]);
    if done.is_empty() {
        return Err(format!("no font table entry named {from}"));
    }
    Ok((out, done))
}

/// Raw name bytes (escaped, as in the file) -> real bytes.
fn unescape(s: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    let mut i = 0;
    while i < s.len() {
        if s[i] == BS && i + 3 < s.len() + 0 && s.get(i + 1) == Some(&b'\'') {
            if let (Some(h), Some(l)) = (s.get(i + 2).and_then(|&c| hexval(c)), s.get(i + 3).and_then(|&c| hexval(c))) {
                v.push(h * 16 + l);
                i += 4;
                continue;
            }
        }
        v.push(s[i]);
        i += 1;
    }
    v
}

/// For one font entry `b[s..e]`: (name start, name end, charset, `fN` index text).
fn name_span(b: &[u8], s: usize, e: usize) -> Option<(usize, usize, i32, String)> {
    let mut i = s + 1;
    let mut charset = 0;
    let mut idx = String::new();
    let mut start: Option<usize> = None;
    while i < e - 1 {
        match b[i] {
            b'{' => {
                if start.is_some() {
                    return Some((start?, i, charset, idx));
                }
                i = group_end(b, i)?;
            }
            b';' => return start.map(|st| (st, i, charset, idx)),
            BS => {
                if b.get(i + 1) == Some(&b'\'') {
                    start.get_or_insert(i);
                    i += 4;
                    continue;
                }
                let mut j = i + 1;
                while j < e && b[j].is_ascii_alphabetic() {
                    j += 1;
                }
                let word = std::str::from_utf8(&b[i + 1..j]).ok()?;
                let ns = j;
                if j < e && (b[j] == b'-' || b[j].is_ascii_digit()) {
                    j += 1;
                    while j < e && b[j].is_ascii_digit() {
                        j += 1;
                    }
                }
                let num = std::str::from_utf8(&b[ns..j]).ok()?;
                if start.is_none() {
                    if word == "fcharset" {
                        charset = num.parse().unwrap_or(0);
                    } else if word == "f" && idx.is_empty() {
                        idx = format!("f{num}");
                    }
                    if j < e && b[j] == b' ' {
                        j += 1; // delimiter space belongs to the control word
                    }
                }
                i = j;
            }
            b'\r' | b'\n' => i += 1,
            _ => {
                start.get_or_insert(i);
                i += 1;
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bs(s: &str) -> String {
        s.replace('~', &(BS as char).to_string())
    }

    #[test]
    fn renames_plain_name_and_keeps_rest() {
        let src = bs("{~rtf1{~fonttbl{~f1~fnil~fcharset134 SimHei{~*~falt x};}{~f2~fnil Arial;}}~f1 body}");
        let (out, done) = rename_font(src.as_bytes(), "simhei", "SimSun").unwrap();
        let s = String::from_utf8(out).unwrap();
        assert_eq!(s, bs("{~rtf1{~fonttbl{~f1~fnil~fcharset134 SimSun{~*~falt x};}{~f2~fnil Arial;}}~f1 body}"));
        assert_eq!(done.len(), 1);
    }

    #[test]
    fn renames_gbk_escaped_name() {
        // GBK bytes of the two ideographs for "hei ti": 0xba 0xda 0xcc 0xe5
        let src = bs("{~rtf1{~fonttbl{~f1~fnil~fcharset134~fprq2{~*~panose 0}~'ba~'da~'cc~'e5{~*~falt SimHei};}}}");
        let (out, done) = rename_font(src.as_bytes(), "\u{9ed1}\u{4f53}", "SimHei").unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains(&bs("{~*~panose 0}SimHei{~*~falt SimHei};")), "{s}");
        assert_eq!(done[0].index, "f1");
    }

    #[test]
    fn missing_name_is_error() {
        let src = bs("{~rtf1{~fonttbl{~f1~fnil Arial;}}}");
        assert!(rename_font(src.as_bytes(), "SimHei", "x").is_err());
    }
}
