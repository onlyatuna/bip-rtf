mod edit;
mod image;
mod render;
mod rtf;
mod sample;
mod tagedit;

use rtf::{Doc, Tag, collect_tags, is_cjk, line_of, parse};
use std::collections::{BTreeMap, BTreeSet};
use std::process::ExitCode;

const USAGE: &str = "bip-rtf: inspect and make small edits to Oracle BI Publisher RTF templates

usage:
  bip-rtf tags  FILE [--json]        list every template tag, in document order
  bip-rtf fonts FILE                  fonts used by CJK text and by other text
  bip-rtf lint  FILE [--xml SAMPLE]   check tags; exit 1 if there are errors
  bip-rtf set-font FILE --from NAME --to NAME -o OUT
                                      rename font table entries into a NEW file; the tag list,
                                      docvars and text must stay identical or nothing is written
  bip-rtf set-tag FILE --from '<?OLD?>' --to '<?NEW?>' -o OUT
                                      rename one tag (docvar, typed text, and a simple field's label) into a NEW file
  bip-rtf images FILE                list pictures: number, kind, pixel size, display size
  bip-rtf set-image FILE --image N --png NEW.png -o OUT [--stretch]
                                      replace the data of PNG picture N (1-based) into a NEW file
  bip-rtf sample FILE [-o OUT.xml] [--rows N] [--root NAME]
                                      sample data XML guessed from the template (loops, fields, image paths)
  bip-rtf render FILE --xml DATA.xml -o OUT.pdf [--tb DIR] [--java EXE] [--locale L] [--config DIR]
                                      [--memory MB] [--timeout SEC] [--keep]
                                      make a PDF with the Template Builder engine (env: BIPRTF_TB_HOME, BIPRTF_JAVA)
";

/// On a Windows console that is not using UTF-8, switch the output code page to UTF-8 while the
/// program runs and restore it afterwards, so CJK text is not shown as garbage. No effect elsewhere.
struct ConsoleCp(#[allow(dead_code)] Option<u32>);

#[cfg(windows)]
mod win {
    unsafe extern "system" {
        pub fn GetConsoleOutputCP() -> u32;
        pub fn SetConsoleOutputCP(cp: u32) -> i32;
    }
}

impl ConsoleCp {
    fn utf8() -> Self {
        #[cfg(windows)]
        {
            use std::io::IsTerminal;
            if std::io::stdout().is_terminal() {
                // SAFETY: plain Win32 calls with no pointers.
                let old = unsafe { win::GetConsoleOutputCP() };
                if old != 65001 && unsafe { win::SetConsoleOutputCP(65001) } != 0 {
                    return ConsoleCp(Some(old));
                }
            }
        }
        ConsoleCp(None)
    }
}

impl Drop for ConsoleCp {
    fn drop(&mut self) {
        #[cfg(windows)]
        if let Some(old) = self.0 {
            // SAFETY: as above.
            unsafe { win::SetConsoleOutputCP(old) };
        }
    }
}

fn main() -> ExitCode {
    let _cp = ConsoleCp::utf8();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("bip-rtf: {e}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[String]) -> Result<ExitCode, String> {
    let Some(cmd) = args.first() else {
        print!("{USAGE}");
        return Ok(ExitCode::from(2));
    };
    if matches!(cmd.as_str(), "help" | "--help" | "-h") {
        print!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    }
    if matches!(cmd.as_str(), "version" | "--version") {
        println!("bip-rtf {}", env!("CARGO_PKG_VERSION"));
        return Ok(ExitCode::SUCCESS);
    }
    let mut file: Option<&str> = None;
    let mut json = false;
    let mut xml: Option<&str> = None;
    let (mut from, mut to, mut outp): (Option<&str>, Option<&str>, Option<&str>) = (None, None, None);
    let (mut rows, mut root, mut java, mut tb, mut config) = (3usize, "DATA", None::<&str>, None::<&str>, None::<&str>);
    let (mut locale, mut keep) = ("en-US", false);
    let (mut memory_mb, mut timeout_s) = (256u32, 180u64);
    let (mut image_n, mut png, mut stretch) = (None::<usize>, None::<&str>, false);
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--json" => json = true,
            "--xml" => xml = Some(it.next().ok_or("--xml needs a file")?.as_str()),
            "--from" => from = Some(it.next().ok_or("--from needs a name")?.as_str()),
            "--to" => to = Some(it.next().ok_or("--to needs a name")?.as_str()),
            "--rows" => rows = it.next().ok_or("--rows needs a number")?.parse().map_err(|_| "--rows needs a number")?,
            "--root" => root = it.next().ok_or("--root needs a name")?.as_str(),
            "--java" => java = Some(it.next().ok_or("--java needs a path")?.as_str()),
            "--tb" => tb = Some(it.next().ok_or("--tb needs a folder")?.as_str()),
            "--config" => config = Some(it.next().ok_or("--config needs a folder")?.as_str()),
            "--locale" => locale = it.next().ok_or("--locale needs a value")?.as_str(),
            "--keep" => keep = true,
            "--stretch" => stretch = true,
            "--memory" => memory_mb = it.next().ok_or("--memory needs MB")?.parse().map_err(|_| "--memory needs a number of MB")?,
            "--timeout" => timeout_s = it.next().ok_or("--timeout needs seconds")?.parse().map_err(|_| "--timeout needs a number of seconds")?,
            "--image" => image_n = Some(it.next().ok_or("--image needs a number")?.parse().map_err(|_| "--image needs a number")?),
            "--png" => png = Some(it.next().ok_or("--png needs a file")?.as_str()),
            "-o" | "--out" => outp = Some(it.next().ok_or("-o needs a file")?.as_str()),
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            s => file = Some(s),
        }
    }
    let path = file.ok_or("missing FILE")?;
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    if !bytes.starts_with(b"{\\rtf") {
        return Err(format!("{path}: not an RTF file"));
    }
    let doc = parse(&bytes);
    match cmd.as_str() {
        "tags" => cmd_tags(path, &doc, json),
        "fonts" => cmd_fonts(path, &doc),
        "lint" => return cmd_lint(path, &doc, xml),
        "images" => cmd_images(path, &bytes),
        "set-image" => {
            let n = image_n.ok_or("set-image needs --image N")?;
            let (pf, o) = (png.ok_or("set-image needs --png NEW.png")?, outp.ok_or("set-image needs -o OUT (it never edits in place)")?);
            return cmd_set_image(path, &bytes, &doc, n, pf, o, stretch);
        }
        "set-tag" => {
            let (f, t, o) = (
                from.ok_or("set-tag needs --from")?,
                to.ok_or("set-tag needs --to")?,
                outp.ok_or("set-tag needs -o OUT (it never edits in place)")?,
            );
            return cmd_set_tag(path, &bytes, &doc, f, t, o);
        }
        "sample" => return cmd_sample(path, &doc, root, rows, outp),
        "render" => {
            let (x, o) = (xml.ok_or("render needs --xml DATA.xml")?, outp.ok_or("render needs -o OUT.pdf")?);
            let opts = render::Opts { java: java.map(String::from), tb: tb.map(String::from), locale: locale.to_string(), config: config.map(String::from), keep, memory_mb, timeout_s };
            println!("{}", render::render(path, x, o, &opts)?);
        }
        "set-font" => {
            let (f, t, o) = (
                from.ok_or("set-font needs --from")?,
                to.ok_or("set-font needs --to")?,
                outp.ok_or("set-font needs -o OUT (it never edits in place)")?,
            );
            return cmd_set_font(path, &bytes, &doc, f, t, o);
        }
        other => return Err(format!("unknown command {other}\n{USAGE}")),
    }
    Ok(ExitCode::SUCCESS)
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn cmd_tags(path: &str, d: &Doc, json: bool) {
    let tags = collect_tags(d);
    if json {
        let items: Vec<String> = tags
            .iter()
            .map(|t| {
                format!(
                    "{{\"line\":{},\"source\":{},\"tag\":{},\"problem\":{}}}",
                    line_of(d, t.pos),
                    json_str(t.src),
                    json_str(&t.text),
                    t.problem.as_deref().map_or("null".to_string(), json_str)
                )
            })
            .collect();
        println!("{{\"file\":{},\"tags\":[{}]}}", json_str(path), items.join(","));
        return;
    }
    let forms = tags.iter().filter(|t| t.src == "form").count();
    println!("{path}: {} tags (form fields {forms}, typed text {}), docvars {}, pictures {}", tags.len(), tags.len() - forms, d.docvars.len(), d.picts);
    for (n, t) in tags.iter().enumerate() {
        let mark = t.problem.as_ref().map_or(String::new(), |p| format!("   !! {p}"));
        println!("{:>3}  L{:<3} {:<4} {}{}", n + 1, line_of(d, t.pos), t.src, t.text, mark);
    }
}

type Usage = BTreeMap<String, (usize, String)>;

/// font name -> (count, sample text) for CJK chars and for the rest.
fn font_usage(d: &Doc) -> (Usage, Usage) {
    let mut cjk: Usage = BTreeMap::new();
    let mut other: Usage = BTreeMap::new();
    for (c, &f) in d.text.iter().zip(&d.fonts_of) {
        if c.is_whitespace() {
            continue;
        }
        let map = if is_cjk(*c) { &mut cjk } else { &mut other };
        let e = map.entry(d.font_name(f)).or_default();
        e.0 += 1;
        if e.1.chars().count() < 6 {
            e.1.push(*c);
        }
    }
    (cjk, other)
}

fn cmd_fonts(path: &str, d: &Doc) {
    let (cjk, other) = font_usage(d);
    println!("{path}: font table {} entries", d.fonttbl.len());
    println!("CJK text:");
    if cjk.is_empty() {
        println!("  (none)");
    }
    for (n, (cnt, sample)) in &cjk {
        println!("  {n:<24} {cnt:>5} chars  e.g. {sample}");
    }
    println!("Other text:");
    for (n, (cnt, sample)) in &other {
        println!("  {n:<24} {cnt:>5} chars  e.g. {sample}");
    }
}

/// Everything that must not change when only fonts are renamed.
fn fingerprint(d: &Doc) -> (Vec<String>, Vec<(String, String)>, Vec<char>, usize) {
    (collect_tags(d).into_iter().map(|t| t.text).collect(), d.docvars.clone(), d.text.clone(), d.picts)
}

fn cmd_images(path: &str, bytes: &[u8]) {
    let pics = image::scan(bytes);
    println!("{path}: {} pictures", pics.len());
    for (i, p) in pics.iter().enumerate() {
        let size = p.end - p.data_start;
        let note = format!("{}{}", if p.shape_fill { "  [fills a drawing shape]" } else { "" }, if p.crop { "  [cropped]" } else { "" });
        println!(
            "{:>3}  {:<10} {}x{} px   shown {:.1} x {:.1} cm   data {} bytes{}",
            i + 1,
            p.kind,
            p.w,
            p.h,
            p.wg as f64 / 567.0,
            p.hg as f64 / 567.0,
            size,
            note
        );
    }
}

fn cmd_set_image(path: &str, bytes: &[u8], before: &Doc, n: usize, png: &str, out: &str, stretch: bool) -> Result<ExitCode, String> {
    let same = |a: &str, b: &str| std::fs::canonicalize(a).ok().zip(std::fs::canonicalize(b).ok()).is_some_and(|(x, y)| x == y);
    if out == path || same(path, out) || same(png, out) {
        return Err("-o must be a different file from the input and the PNG".into());
    }
    let data = std::fs::read(png).map_err(|e| format!("{png}: {e}"))?;
    let r = image::replace_png(bytes, n.checked_sub(1).ok_or("--image counts from 1")?, &data, stretch)?;
    let after = parse(&r.bytes);
    if fingerprint(before) != fingerprint(&after) || before.fonttbl.len() != after.fonttbl.len() {
        return Err("safety check failed: tags, docvars, text, pictures or fonts changed. Nothing written.".into());
    }
    let (old, new) = (image::scan(bytes), image::scan(&r.bytes));
    if old.len() != new.len() {
        return Err("safety check failed: the picture count changed. Nothing written.".into());
    }
    for (i, (a, b)) in old.iter().zip(&new).enumerate() {
        let same_pic = if i + 1 == n { image::png_of(&r.bytes, b).as_deref() == Some(data.as_slice()) } else { bytes[a.data_start..a.data_end] == r.bytes[b.data_start..b.data_end] && a.kind == b.kind };
        if !same_pic {
            return Err(format!("safety check failed on picture {}. Nothing written.", i + 1));
        }
    }
    std::fs::write(out, &r.bytes).map_err(|e| format!("{out}: {e}"))?;
    println!("{path} -> {out}");
    println!("  picture {n}: {}x{} px -> {}x{} px; shown {:.1}x{:.1} cm -> {:.1}x{:.1} cm", r.old_px.0, r.old_px.1, r.new_px.0, r.new_px.1, r.old_goal.0 as f64 / 567.0, r.old_goal.1 as f64 / 567.0, r.new_goal.0 as f64 / 567.0, r.new_goal.1 as f64 / 567.0);
    println!("  checked: new picture data equals the PNG; other pictures byte-identical; tags, docvars, text, fonts unchanged");
    if let Some(w) = r.warning {
        println!("  warning: {w}");
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_set_tag(path: &str, bytes: &[u8], before: &Doc, from: &str, to: &str, out: &str) -> Result<ExitCode, String> {
    let same = |a: &str, b: &str| std::fs::canonicalize(a).ok().zip(std::fs::canonicalize(b).ok()).is_some_and(|(x, y)| x == y);
    if out == path || same(path, out) {
        return Err("-o must be a different file from the input".into());
    }
    let (new_bytes, rep) = tagedit::set_tag(bytes, from, to)?;
    let after = parse(&new_bytes);
    let old_tags: Vec<(usize, String)> = collect_tags(before).iter().map(|t| (line_of(before, t.pos), t.text.clone())).collect();
    let new_tags: Vec<String> = collect_tags(&after).into_iter().map(|t| t.text).collect();
    let expected: Vec<String> = old_tags.iter().map(|(_, t)| t.replace(from, to)).collect();
    if let Some(i) = (0..expected.len().max(new_tags.len())).find(|&i| expected.get(i) != new_tags.get(i)) {
        let line = old_tags.get(i).map_or(0, |x| x.0);
        return Err(format!(
            "safety check failed at tag {} (line {line}): expected {:?}, got {:?}. Nothing written. A tag split across Word runs cannot be renamed here; do it in Word.",
            i + 1,
            expected.get(i),
            new_tags.get(i)
        ));
    }
    if before.docvars.len() != after.docvars.len() || before.picts != after.picts || before.fonttbl.len() != after.fonttbl.len() {
        return Err("safety check failed: docvar, picture or font counts changed. Nothing written.".into());
    }
    std::fs::write(out, &new_bytes).map_err(|e| format!("{out}: {e}"))?;
    println!("{path} -> {out}");
    println!("  {from} -> {to}: docvars changed {}, typed occurrences {}, field labels updated {}", rep.docvars, rep.typed, rep.labels);
    for s in &rep.label_skipped {
        println!("  label not updated: {s}");
    }
    println!("  checked: tag list equals the original with the rename applied ({} tags); docvars, pictures, fonts unchanged", new_tags.len());
    Ok(ExitCode::SUCCESS)
}

fn cmd_sample(path: &str, d: &Doc, root: &str, rows: usize, out: Option<&str>) -> Result<ExitCode, String> {
    let tags: Vec<String> = collect_tags(d).into_iter().filter(|t| t.problem.is_none()).map(|t| t.text).collect();
    let docvars: Vec<String> = d.docvars.iter().filter_map(|(n, _)| d.docvar(n)).collect();
    let s = sample::build(&tags, &docvars, root, rows.max(1));
    match out {
        Some(o) => {
            if o == path {
                return Err("-o must be a different file from the input".into());
            }
            std::fs::write(o, &s.xml).map_err(|e| format!("{o}: {e}"))?;
            println!("{path} -> {o}: {} elements, loops {:?}, image paths {:?}", s.fields, s.loops, s.images);
            println!("structure is guessed from the tags and values are made up; a bare field name is placed under <{root}>");
        }
        None => print!("{}", s.xml),
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_set_font(path: &str, bytes: &[u8], before: &Doc, from: &str, to: &str, out: &str) -> Result<ExitCode, String> {
    let same = |a: &str, b: &str| std::fs::canonicalize(a).ok().zip(std::fs::canonicalize(b).ok()).is_some_and(|(x, y)| x == y);
    if out == path || same(path, out) {
        return Err("-o must be a different file from the input".into());
    }
    let (new_bytes, done) = edit::rename_font(bytes, from, to)?;
    let after = parse(&new_bytes);
    if fingerprint(before) != fingerprint(&after) {
        return Err("safety check failed: tags, docvars, text or pictures would change; nothing written".into());
    }
    std::fs::write(out, &new_bytes).map_err(|e| format!("{out}: {e}"))?;
    println!("{path} -> {out}");
    for r in &done {
        println!("  font table entry \\{}: {} -> {}", r.index, r.old, to);
    }
    println!("  checked: {} tags, {} docvars, {} chars, {} pictures all identical", collect_tags(&after).len(), after.docvars.len(), after.text.len(), after.picts);
    println!("  bytes: {} -> {}", bytes.len(), new_bytes.len());
    Ok(ExitCode::SUCCESS)
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
enum Level {
    Error,
    Warn,
    Check,
}

enum Role {
    Open(&'static str),
    Close(&'static str),
    Other,
}

fn inner(tag: &str) -> &str {
    tag.strip_prefix("<?").and_then(|x| x.strip_suffix("?>")).unwrap_or(tag)
}

fn role(inner_text: &str) -> Role {
    let n = inner_text.trim();
    if let Some(rest) = n.strip_prefix("end ") {
        return match rest.trim() {
            "for-each" => Role::Close("for-each"),
            "for-each-group" => Role::Close("for-each-group"),
            "if" => Role::Close("if"),
            "choose" => Role::Close("choose"),
            "when" => Role::Close("when"),
            "template" => Role::Close("template"),
            _ => Role::Other,
        };
    }
    let head = n.split(|c: char| c == ':' || c == '@' || c.is_whitespace()).next().unwrap_or("");
    match head {
        "for-each" => Role::Open("for-each"),
        "for-each-group" => Role::Open("for-each-group"),
        "if" => Role::Open("if"),
        "choose" => Role::Open("choose"),
        "when" => Role::Open("when"),
        "template" => Role::Open("template"),
        _ => Role::Other,
    }
}

fn xml_names(xml: &str) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    let cs: Vec<char> = xml.chars().collect();
    let mut i = 0;
    while i + 1 < cs.len() {
        if cs[i] == '<' && (cs[i + 1].is_alphabetic() || cs[i + 1] == '_') {
            let mut j = i + 1;
            while j < cs.len() && (cs[j].is_alphanumeric() || matches!(cs[j], '_' | '-' | '.' | ':')) {
                j += 1;
            }
            set.insert(cs[i + 1..j].iter().collect());
            i = j;
        } else {
            i += 1;
        }
    }
    set
}

fn is_ident(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// The data field a tag refers to, when that is simple enough to check against a sample XML.
fn data_field(inner_text: &str) -> Option<String> {
    let n = inner_text.trim();
    if is_ident(n) && !matches!(n, "choose" | "otherwise") {
        return Some(n.to_string());
    }
    let (head, rest) = n.split_once(':')?;
    if head.split('@').next() == Some("for-each") {
        let last = rest.trim().rsplit(['/', '.']).next()?;
        if is_ident(last) {
            return Some(last.to_string());
        }
    }
    None
}

fn cmd_lint(path: &str, d: &Doc, xml: Option<&str>) -> Result<ExitCode, String> {
    let tags: Vec<Tag> = collect_tags(d);
    let mut out: Vec<(Level, String)> = Vec::new();
    let mut referenced: BTreeSet<String> = BTreeSet::new();

    for t in &tags {
        let ln = line_of(d, t.pos);
        if let Some(p) = &t.problem {
            out.push((Level::Error, format!("L{ln} {}: {p}", t.text)));
        }
        let body = inner(&t.text);
        if t.text.starts_with("<?") && t.text.ends_with("?>") && body != body.trim() {
            out.push((Level::Check, format!("L{ln} tag has leading/trailing space: {} (engine behaviour not verified)", t.text)));
        }
    }
    for f in &d.forms {
        if let Some(id) = f.stat.trim().strip_prefix("<?ref:").and_then(|x| x.strip_suffix("?>")) {
            referenced.insert(id.to_string());
        }
    }

    // Balanced block tags.
    let mut stack: Vec<(&'static str, usize)> = Vec::new();
    for t in tags.iter().filter(|t| t.problem.is_none()) {
        let ln = line_of(d, t.pos);
        match role(inner(&t.text)) {
            Role::Open(k) => stack.push((k, ln)),
            Role::Close(k) => match stack.last() {
                Some(&(top, _)) if top == k => {
                    stack.pop();
                }
                Some(&(top, tl)) => out.push((Level::Error, format!("L{ln} `end {k}` does not match open `{top}` (L{tl})"))),
                None => out.push((Level::Error, format!("L{ln} `end {k}` has no matching open tag"))),
            },
            Role::Other => {}
        }
    }
    for (k, ln) in stack {
        out.push((Level::Error, format!("L{ln} `{k}` is never closed")));
    }

    // CJK text without a font.
    let (cjk, _) = font_usage(d);
    for (n, (cnt, sample)) in &cjk {
        if n == "(none set)" {
            out.push((Level::Warn, format!("{cnt} CJK chars have no font set (e.g. {sample})")));
        }
    }

    // Unused docvars.
    let unused: Vec<&str> = d
        .docvars
        .iter()
        .map(|(n, _)| n.as_str())
        .filter(|n| n.starts_with("xdo") && n[3..].chars().all(|c| c.is_ascii_digit()) && !referenced.contains(*n))
        .collect();
    if !unused.is_empty() {
        out.push((Level::Check, format!("{} docvar(s) not referenced by any form field: {}", unused.len(), unused.join(", "))));
    }

    // Fields against a sample data XML.
    if let Some(xp) = xml {
        let names = xml_names(&std::fs::read_to_string(xp).map_err(|e| format!("{xp}: {e}"))?);
        let mut seen = BTreeSet::new();
        for t in tags.iter().filter(|t| t.problem.is_none()) {
            if let Some(f) = data_field(inner(&t.text))
                && !names.contains(&f)
                && seen.insert(f.clone())
            {
                out.push((Level::Check, format!("L{} `{f}` not found as an element in {xp}", line_of(d, t.pos))));
            }
        }
    }

    out.sort_by_key(|(l, _)| *l);
    println!("{path}: {} tags, {} CJK font(s)", tags.len(), cjk.len());
    for (lvl, msg) in &out {
        let label = match lvl {
            Level::Error => "ERROR",
            Level::Warn => "WARN ",
            Level::Check => "CHECK",
        };
        println!("[{label}] {msg}");
    }
    let n = |l: Level| out.iter().filter(|(x, _)| *x == l).count();
    println!("=> {} error(s), {} warning(s), {} to check", n(Level::Error), n(Level::Warn), n(Level::Check));
    Ok(if n(Level::Error) > 0 { ExitCode::from(1) } else { ExitCode::SUCCESS })
}
