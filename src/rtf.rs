//! Minimal read-only RTF scanner for BI Publisher templates.
//!
//! It extracts what a template author cares about and nothing else: the visible text (with the
//! font each character is set in), the font table, the `\docvar` table (where BI Publisher keeps
//! the real tag text, Base64-encoded), and Word legacy form fields (which carry a `<?ref:xdoNNNN?>`
//! pointer into that table). It never writes RTF.

use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Body,
    Skip,
    FontTbl,
    FfDef,
    FfStat,
    Docvar,
    DocvarPart,
}

#[derive(Clone)]
struct St {
    kind: Kind,
    /// `\fN`
    f: i32,
    /// Associated fonts by class: 0 = loch, 1 = hich, 2 = dbch, 3 = rtlch.
    assoc: [i32; 4],
    class: usize,
    /// `\ucN`: how many fallback characters follow a `\uN`.
    uc: usize,
    charset: i32,
}

impl St {
    fn new() -> Self {
        St { kind: Kind::Body, f: -1, assoc: [-1; 4], class: 0, uc: 1, charset: 0 }
    }
}

/// A Word legacy form field. BI Publisher stores `<?ref:xdoNNNN?>` as its status text.
#[derive(Default, Debug)]
pub struct Form {
    pub pos: usize,
    pub deftext: String,
    pub stat: String,
}

#[derive(Default, Debug)]
pub struct Doc {
    pub text: Vec<char>,
    /// Effective font index for each char of `text` (-1 = none set).
    pub fonts_of: Vec<i32>,
    /// font index -> (name, charset)
    pub fonttbl: HashMap<i32, (String, i32)>,
    /// (name, decoded value) pairs of `{\*\docvar {name}{value}}`; values are still Base64.
    pub docvars: Vec<(String, String)>,
    pub forms: Vec<Form>,
    pub deff: i32,
    pub codepage: u32,
    pub fldinst: usize,
    pub picts: usize,
}

impl Doc {
    pub fn font_name(&self, idx: i32) -> String {
        let idx = if idx < 0 { self.deff } else { idx };
        match self.fonttbl.get(&idx) {
            Some((n, _)) => n.clone(),
            None => "(none set)".to_string(),
        }
    }

    /// Text of the docvar whose name is exactly `id`, Base64-decoded.
    pub fn docvar(&self, id: &str) -> Option<String> {
        self.docvars
            .iter()
            .find(|(n, _)| n == id)
            .map(|(_, v)| String::from_utf8_lossy(&b64_decode(v)).into_owned())
    }
}

pub fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x2E80..=0x9FFF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF | 0x20000..=0x2FFFF)
}

pub fn b64_decode(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        buf = (buf << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
            buf &= (1u32 << bits) - 1;
        }
    }
    out
}

pub fn encoding_for_charset(cs: i32) -> Option<&'static encoding_rs::Encoding> {
    match cs {
        134 => Some(encoding_rs::GBK),
        136 => Some(encoding_rs::BIG5),
        128 => Some(encoding_rs::SHIFT_JIS),
        129 => Some(encoding_rs::EUC_KR),
        161 => Some(encoding_rs::WINDOWS_1253),
        162 => Some(encoding_rs::WINDOWS_1254),
        163 => Some(encoding_rs::WINDOWS_1258),
        177 => Some(encoding_rs::WINDOWS_1255),
        178 => Some(encoding_rs::WINDOWS_1256),
        186 => Some(encoding_rs::WINDOWS_1257),
        204 => Some(encoding_rs::WINDOWS_1251),
        222 => Some(encoding_rs::WINDOWS_874),
        238 => Some(encoding_rs::WINDOWS_1250),
        _ => None,
    }
}

fn encoding_for_codepage(cp: u32) -> &'static encoding_rs::Encoding {
    match cp {
        950 => encoding_rs::BIG5,
        936 => encoding_rs::GBK,
        932 => encoding_rs::SHIFT_JIS,
        949 => encoding_rs::EUC_KR,
        874 => encoding_rs::WINDOWS_874,
        1250 => encoding_rs::WINDOWS_1250,
        1251 => encoding_rs::WINDOWS_1251,
        1253 => encoding_rs::WINDOWS_1253,
        1254 => encoding_rs::WINDOWS_1254,
        1255 => encoding_rs::WINDOWS_1255,
        1256 => encoding_rs::WINDOWS_1256,
        1257 => encoding_rs::WINDOWS_1257,
        1258 => encoding_rs::WINDOWS_1258,
        _ => encoding_rs::WINDOWS_1252,
    }
}

struct P<'a> {
    b: &'a [u8],
    i: usize,
    stack: Vec<St>,
    doc: Doc,
    cap: String,
    parts: Vec<String>,
    pending: Vec<u8>,
    skip: usize,
    hi: Option<u32>,
    star: bool,
}

pub fn parse(bytes: &[u8]) -> Doc {
    let mut p = P {
        b: bytes,
        i: 0,
        stack: vec![St::new()],
        doc: Doc { deff: 0, codepage: 1252, ..Default::default() },
        cap: String::new(),
        parts: Vec::new(),
        pending: Vec::new(),
        skip: 0,
        hi: None,
        star: false,
    };
    p.run();
    p.doc
}

impl P<'_> {
    fn top(&mut self) -> &mut St {
        if self.stack.is_empty() {
            self.stack.push(St::new());
        }
        let n = self.stack.len();
        &mut self.stack[n - 1]
    }

    fn run(&mut self) {
        while self.i < self.b.len() {
            let c = self.b[self.i];
            match c {
                b'{' => {
                    self.flush();
                    self.push_group();
                    self.i += 1;
                }
                b'}' => {
                    self.flush();
                    self.pop_group();
                    self.i += 1;
                }
                b'\\' => self.control(),
                b'\r' | b'\n' => self.i += 1,
                _ => {
                    self.i += 1;
                    if c >= 0x80 {
                        if self.skip > 0 {
                            self.skip -= 1;
                        } else {
                            self.pending.push(c);
                        }
                    } else {
                        self.flush();
                        self.literal(c);
                    }
                }
            }
        }
        self.flush();
    }

    fn literal(&mut self, c: u8) {
        if self.skip > 0 {
            self.skip -= 1;
            return;
        }
        self.put(c as char);
    }

    fn push_group(&mut self) {
        let mut t = self.top().clone();
        if t.kind == Kind::Docvar {
            t.kind = Kind::DocvarPart;
            self.cap.clear();
        }
        self.stack.push(t);
    }

    fn pop_group(&mut self) {
        if self.stack.len() <= 1 {
            return;
        }
        let Some(t) = self.stack.pop() else { return };
        match t.kind {
            Kind::FfDef => {
                let s = std::mem::take(&mut self.cap);
                if let Some(f) = self.doc.forms.last_mut() {
                    f.deftext = s;
                }
            }
            Kind::FfStat => {
                let s = std::mem::take(&mut self.cap);
                if let Some(f) = self.doc.forms.last_mut() {
                    f.stat = s;
                }
            }
            Kind::DocvarPart => {
                let s = std::mem::take(&mut self.cap);
                self.parts.push(s);
            }
            Kind::Docvar => {
                let mut it = std::mem::take(&mut self.parts).into_iter();
                if let (Some(n), Some(v)) = (it.next(), it.next()) {
                    self.doc.docvars.push((n, v));
                }
            }
            _ => {}
        }
    }

    /// Decode pending `\'hh` bytes with the charset of the font table entry, or the document codepage.
    fn flush(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let bytes = std::mem::take(&mut self.pending);
        // `\'hh` bytes are in the code page of the font they are set in (its `\fcharset`),
        // not necessarily the document's `\ansicpg`.
        let (kind, cs, font) = {
            let t = self.top();
            (t.kind, t.charset, if t.assoc[2] >= 0 { t.assoc[2] } else { t.f })
        };
        let cs = if kind == Kind::FontTbl { cs } else { self.doc.fonttbl.get(&font).map_or(0, |e| e.1) };
        let enc = encoding_for_charset(cs).unwrap_or_else(|| encoding_for_codepage(self.doc.codepage));
        let (s, _, _) = enc.decode(&bytes);
        for c in s.chars() {
            self.put(c);
        }
    }

    /// One decoded character of text.
    fn put(&mut self, ch: char) {
        let (kind, eff) = {
            let t = self.top();
            let eff = if is_cjk(ch) && t.assoc[2] >= 0 { t.assoc[2] } else { t.f };
            (t.kind, eff)
        };
        match kind {
            Kind::Body => {
                self.doc.text.push(ch);
                self.doc.fonts_of.push(eff);
            }
            Kind::FontTbl => {
                if ch == ';' {
                    self.finish_font();
                } else {
                    self.cap.push(ch);
                }
            }
            Kind::FfDef | Kind::FfStat | Kind::DocvarPart => self.cap.push(ch),
            Kind::Skip | Kind::Docvar => {}
        }
    }

    /// Control characters (paragraph, tab, ...) only count in the body.
    fn put_ctl(&mut self, ch: char) {
        if self.top().kind == Kind::Body {
            self.put(ch);
        }
    }

    fn finish_font(&mut self) {
        let (idx, cs) = {
            let t = self.top();
            (t.f, t.charset)
        };
        let name = self.cap.trim().to_string();
        self.cap.clear();
        if idx >= 0 && !name.is_empty() {
            self.doc.fonttbl.insert(idx, (name, cs));
        }
    }

    fn control(&mut self) {
        self.i += 1;
        let Some(&c) = self.b.get(self.i) else { return };
        if c != b'\'' {
            self.flush();
        }
        if c.is_ascii_alphabetic() {
            let s = self.i;
            while self.i < self.b.len() && self.b[self.i].is_ascii_alphabetic() {
                self.i += 1;
            }
            let name = String::from_utf8_lossy(&self.b[s..self.i]).into_owned();
            let mut neg = false;
            if self.b.get(self.i) == Some(&b'-') && self.b.get(self.i + 1).is_some_and(u8::is_ascii_digit) {
                neg = true;
                self.i += 1;
            }
            let ds = self.i;
            while self.i < self.b.len() && self.b[self.i].is_ascii_digit() {
                self.i += 1;
            }
            let num = if self.i > ds {
                std::str::from_utf8(&self.b[ds..self.i]).ok().and_then(|x| x.parse::<i32>().ok()).map(|n| if neg { -n } else { n })
            } else {
                None
            };
            if self.b.get(self.i) == Some(&b' ') {
                self.i += 1;
            }
            self.word(&name, num);
            return;
        }
        self.i += 1;
        match c {
            b'\'' => {
                let h = self.b.get(self.i..self.i + 2).and_then(|x| std::str::from_utf8(x).ok()).and_then(|x| u8::from_str_radix(x, 16).ok());
                self.i += 2;
                if let Some(byte) = h {
                    if self.skip > 0 {
                        self.skip -= 1;
                    } else {
                        self.pending.push(byte);
                    }
                }
            }
            b'*' => self.star = true,
            b'\\' | b'{' | b'}' => self.literal(c),
            b'~' => self.put('\u{a0}'),
            b'\n' | b'\r' => self.put_ctl('\n'),
            _ => {}
        }
    }

    fn word(&mut self, name: &str, num: Option<i32>) {
        if self.star {
            self.star = false;
            match name {
                "formfield" => {
                    let pos = self.doc.text.len();
                    self.doc.forms.push(Form { pos, ..Default::default() });
                    self.top().kind = Kind::Skip;
                }
                "ffdeftext" => {
                    self.cap.clear();
                    self.top().kind = Kind::FfDef;
                }
                "ffstattext" => {
                    self.cap.clear();
                    self.top().kind = Kind::FfStat;
                }
                "docvar" => {
                    self.parts.clear();
                    self.top().kind = Kind::Docvar;
                }
                "fldinst" => {
                    self.doc.fldinst += 1;
                    self.top().kind = Kind::Skip;
                }
                _ => self.top().kind = Kind::Skip,
            }
            return;
        }
        match name {
            "fonttbl" => {
                self.cap.clear();
                self.top().kind = Kind::FontTbl;
            }
            "colortbl" | "stylesheet" | "info" | "listtable" | "listoverridetable" | "rsidtbl" | "filetbl" | "revtbl" | "listtext" | "pntext"
            | "nonshppict" => self.top().kind = Kind::Skip,
            "pict" => {
                self.doc.picts += 1;
                self.top().kind = Kind::Skip;
            }
            "f" => self.top().f = num.unwrap_or(-1),
            "af" => {
                let t = self.top();
                let c = t.class;
                t.assoc[c] = num.unwrap_or(-1);
            }
            "loch" | "ltrch" => self.top().class = 0,
            "hich" => self.top().class = 1,
            "dbch" => self.top().class = 2,
            "rtlch" => self.top().class = 3,
            "fcharset" => self.top().charset = num.unwrap_or(0),
            "uc" => self.top().uc = num.unwrap_or(1).max(0) as usize,
            "u" => self.unicode(num.unwrap_or(0)),
            "ansicpg" => self.doc.codepage = num.unwrap_or(1252).max(0) as u32,
            "deff" => self.doc.deff = num.unwrap_or(0),
            "plain" => {
                let t = self.top();
                t.f = -1;
                t.assoc = [-1; 4];
            }
            "par" | "line" | "sect" | "page" | "row" => self.put_ctl('\n'),
            "tab" | "cell" => self.put_ctl('\t'),
            _ => {}
        }
    }

    fn unicode(&mut self, n: i32) {
        let v = if n < 0 { (n + 65536) as u32 } else { n as u32 };
        let skip = self.top().uc;
        if (0xD800..0xDC00).contains(&v) {
            self.hi = Some(v);
            self.skip = skip;
            return;
        }
        let ch = if (0xDC00..0xE000).contains(&v) {
            self.hi.take().and_then(|h| char::from_u32(0x10000 + ((h - 0xD800) << 10) + (v - 0xDC00)))
        } else {
            char::from_u32(v)
        };
        if let Some(c) = ch {
            self.put(c);
        }
        self.skip = skip;
    }
}

/// A BI Publisher tag found in the template, in document order.
#[derive(Debug)]
pub struct Tag {
    pub pos: usize,
    /// `"form"` for a legacy form field, `"text"` for a tag typed into the document.
    pub src: &'static str,
    /// Full tag text, `<?...?>`.
    pub text: String,
    pub problem: Option<String>,
}

pub fn collect_tags(d: &Doc) -> Vec<Tag> {
    let mut v: Vec<Tag> = Vec::new();
    let t = &d.text;
    let mut i = 0;
    while i + 1 < t.len() {
        if t[i] == '<' && t[i + 1] == '?' {
            let mut j = i + 2;
            let mut end = None;
            while j + 1 < t.len() {
                if t[j] == '?' && t[j + 1] == '>' {
                    end = Some(j);
                    break;
                }
                j += 1;
            }
            match end {
                Some(e) => {
                    v.push(Tag { pos: i, src: "text", text: t[i..e + 2].iter().collect(), problem: None });
                    i = e + 2;
                }
                None => {
                    let tail: String = t[i..t.len().min(i + 30)].iter().collect();
                    v.push(Tag { pos: i, src: "text", text: tail, problem: Some("unterminated tag, missing ?>".into()) });
                    break;
                }
            }
        } else {
            i += 1;
        }
    }
    for f in &d.forms {
        let stat = f.stat.trim();
        let (text, problem) = match stat.strip_prefix("<?ref:").and_then(|x| x.strip_suffix("?>")) {
            Some(id) => match d.docvar(id) {
                Some(s) => (s, None),
                None => (stat.to_string(), Some(format!("docvar {id} not found"))),
            },
            None if stat.starts_with("<?") => (stat.to_string(), None),
            None => (stat.to_string(), Some(format!("form field has no tag text (shown text: {})", f.deftext))),
        };
        let text = text.trim().to_string();
        // One form field may hold several tags (e.g. `<?split-by-page-break:?>` + `<?end for-each?>`).
        let parts = split_tags(&text);
        if problem.is_none() && parts.len() > 1 {
            for p in parts {
                v.push(Tag { pos: f.pos, src: "form", text: p, problem: None });
            }
        } else {
            v.push(Tag { pos: f.pos, src: "form", text, problem });
        }
    }
    v.sort_by_key(|t| t.pos);
    v
}

/// Each complete `<?...?>` in `s`, in order.
fn split_tags(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(a) = rest.find("<?") {
        let Some(b) = rest[a + 2..].find("?>") else { break };
        let end = a + 2 + b + 2;
        out.push(rest[a..end].to_string());
        rest = &rest[end..];
    }
    out
}

/// 1-based line (paragraph/cell row) number of a text offset.
pub fn line_of(d: &Doc, pos: usize) -> usize {
    d.text.iter().take(pos).filter(|&&c| c == '\n').count() + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charsets_map_to_encodings() {
        assert_eq!(encoding_for_charset(204), Some(encoding_rs::WINDOWS_1251));
        assert_eq!(encoding_for_charset(134), Some(encoding_rs::GBK));
        assert_eq!(encoding_for_charset(0), None);
        assert_eq!(encoding_for_codepage(1251), encoding_rs::WINDOWS_1251);
        assert_eq!(encoding_for_codepage(65001), encoding_rs::WINDOWS_1252);
    }

    #[test]
    fn base64_decodes_tag() {
        assert_eq!(String::from_utf8_lossy(&b64_decode("PD9mb3ItZWFjaDpMSU5FUz8+")), "<?for-each:LINES?>");
        assert_eq!(String::from_utf8_lossy(&b64_decode("PD9zdGFydGVyPz4=")), "<?starter?>");
    }

    #[test]
    fn plain_tag_split_across_runs() {
        let rtf = br"{\rtf1\ansi\ansicpg1252\deff0{\fonttbl{\f0\fnil Arial;}}\pard {\f0 <?PHONE_A}{\b 1}{\f0 ?>}\par}";
        let d = parse(rtf);
        let tags = collect_tags(&d);
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].text, "<?PHONE_A1?>");
        assert_eq!(d.font_name(0), "Arial");
    }

    #[test]
    fn form_field_ref_resolves_through_docvar() {
        let rtf = br"{\rtf1\ansi{\*\docvar {xdo0001}{PD9mb3ItZWFjaDpMSU5FUz8+}}\pard {\field{\*\fldinst {FORMTEXT}}{\*\formfield{\fftype0{\*\ffdeftext for-each LINES}{\*\ffstattext <?ref:xdo0001?>}}}{\fldrslt for-each LINES}}\par}";
        let d = parse(rtf);
        let tags = collect_tags(&d);
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].src, "form");
        assert_eq!(tags[0].text, "<?for-each:LINES?>");
        assert!(tags[0].problem.is_none());
    }

    #[test]
    fn unresolved_ref_is_reported() {
        let rtf = br"{\rtf1\ansi{\*\formfield{\fftype0{\*\ffstattext <?ref:xdo9999?>}}}}";
        let tags = collect_tags(&parse(rtf));
        assert_eq!(tags.len(), 1);
        assert!(tags[0].problem.is_some());
    }

    #[test]
    fn cjk_font_follows_dbch_associated_font() {
        let bs = char::from(92u8);
        let rtf = format!("{{{bs}rtf1{bs}ansi{bs}deff0{{{bs}fonttbl{{{bs}f0 Arial;}}{{{bs}f1 SimHei;}}}}{bs}pard{bs}loch{bs}f0{bs}dbch{bs}af1 {bs}u20013?{bs}u25991?{bs}par}}");
        let d = parse(rtf.as_bytes());
        assert_eq!(d.text.iter().collect::<String>(), "\u{4e2d}\u{6587}\n");
        assert_eq!(d.font_name(d.fonts_of[0]), "SimHei");
    }

    #[test]
    fn unterminated_tag_is_a_problem() {
        let d = parse(br"{\rtf1\ansi \pard <?if:A\par}");
        let tags = collect_tags(&d);
        assert_eq!(tags.len(), 1);
        assert!(tags[0].problem.is_some());
    }
}
