//! Sample data XML guessed from a template's tags.
//!
//! The structure comes only from what the template says: loops (`for-each`), field names in tags and
//! expressions, absolute paths such as `//DOC/PICTURE`, and image blocks (`fo:instream-foreign-object`
//! with its `content-type` path and its data field). Values are made up. Where a template says nothing
//! about where a bare field name lives, it goes under the root element.

use std::collections::BTreeSet;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Kind {
    Text,
    Num,
    ImgType,
    ImgData,
}

#[derive(Default)]
struct E {
    name: String,
    kids: Vec<E>,
    leaf: Option<Kind>,
    repeat: usize,
}

pub struct Sample {
    pub xml: String,
    pub fields: usize,
    pub loops: Vec<String>,
    pub images: Vec<String>,
}

const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

const KEYWORDS: &[&str] = &[
    "if", "then", "else", "and", "or", "not", "div", "mod", "true", "false", "end", "for", "in", "return", "some", "every", "satisfies", "to", "eq", "ne", "lt",
    "le", "gt", "ge", "let", "text", "node", "last", "position",
];
const NUMERIC_FNS: &[&str] = &["sum", "number", "format-number", "round", "floor", "ceiling", "avg", "min", "max", "tochecknumber"];
const SKIP_HEADS: &[&str] = &[
    "call-template", "ref", "import", "param", "variable", "choose", "otherwise", "start", "section", "split-by-page-break", "apply-templates", "sort",
];

/// Field names in an expression, each with a flag for "used as a number".
pub fn idents(s: &str) -> Vec<(String, bool)> {
    let cs: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if c == '\'' || c == '"' {
            i += 1;
            while i < cs.len() && cs[i] != c {
                i += 1;
            }
            i += 1;
        } else if c == '(' {
            stack.push(String::new());
            i += 1;
        } else if c == ')' {
            stack.pop();
            i += 1;
        } else if c.is_ascii_digit() {
            while i < cs.len() && (cs[i].is_ascii_digit() || cs[i] == '.') {
                i += 1;
            }
        } else if c.is_alphabetic() || matches!(c, '_' | '/' | '@') {
            let st = i;
            while i < cs.len() && (cs[i].is_alphanumeric() || matches!(cs[i], '_' | '-' | '.' | '/' | '@')) {
                i += 1;
            }
            let t: String = cs[st..i].iter().collect();
            let mut j = i;
            while j < cs.len() && cs[j] == ' ' {
                j += 1;
            }
            let next = cs.get(j).copied();
            if next == Some(':') && cs.get(j + 1) != Some(&':') && !t.contains('/') {
                i = j + 1; // namespace or directive prefix
            } else if next == Some('(') {
                stack.push(t.to_lowercase());
                i = j + 1;
            } else if t.starts_with('@') || t.trim_matches('.').is_empty() || KEYWORDS.contains(&t.to_lowercase().as_str()) && !t.contains('/') {
                // attribute, dot, or keyword
            } else {
                out.push((t, stack.iter().any(|f| NUMERIC_FNS.contains(&f.as_str()))));
            }
        } else {
            i += 1;
        }
    }
    out
}

fn clean(seg: &str) -> String {
    let seg = seg.split('[').next().unwrap_or("");
    seg.chars().filter(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.')).collect()
}

fn resolve(ctx: &[String], path: &str, root: &str) -> Vec<String> {
    let abs = path.starts_with('/');
    let mut base: Vec<String> = if abs { Vec::new() } else { ctx.to_vec() };
    let mut first = true;
    for raw in path.split('/').filter(|s| !s.is_empty()) {
        if raw.starts_with('@') {
            continue;
        }
        if raw == ".." {
            base.pop();
            continue;
        }
        let s = clean(raw);
        if s.is_empty() || s == "." || s.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            continue;
        }
        if abs && first && s == root {
            first = false;
            continue;
        }
        first = false;
        base.push(s);
    }
    base
}

fn ensure<'a>(root: &'a mut E, segs: &[String]) -> &'a mut E {
    let mut cur = root;
    for s in segs {
        let idx = match cur.kids.iter().position(|k| &k.name == s) {
            Some(i) => i,
            None => {
                cur.kids.push(E { name: s.clone(), ..E::default() });
                cur.kids.len() - 1
            }
        };
        cur.leaf = None; // an element with children is not a leaf
        cur = &mut cur.kids[idx];
    }
    cur
}

fn add_leaf(root: &mut E, ctx: &[String], path: &str, rootname: &str, kind: Kind) {
    let segs = resolve(ctx, path, rootname);
    let Some((last, parent)) = segs.split_last() else { return };
    let p = ensure(root, parent);
    let idx = match p.kids.iter().position(|k| &k.name == last) {
        Some(i) => i,
        None => {
            p.kids.push(E { name: last.clone(), ..E::default() });
            p.kids.len() - 1
        }
    };
    p.leaf = None;
    let k = &mut p.kids[idx];
    if k.kids.is_empty() && k.repeat == 0 {
        k.leaf = Some(k.leaf.map_or(kind, |old| old.max(kind)));
    }
}

fn enter_loop(root: &mut E, ctx: &[String], path: &str, rootname: &str, rows: usize) -> Vec<String> {
    let segs = resolve(ctx, path, rootname);
    if segs.is_empty() {
        return ctx.to_vec();
    }
    let e = ensure(root, &segs);
    e.leaf = None;
    e.repeat = rows;
    segs
}

/// Image blocks inside a text: the `content-type="{path}"` path and the first `<?field?>` after it.
fn scan_images(text: &str, root: &mut E, ctx: &[String], rootname: &str, images: &mut Vec<(String, bool)>) {
    let mut from = 0;
    while let Some(p) = text[from..].find("content-type=\"{") {
        let start = from + p + "content-type=\"{".len();
        let Some(len) = text[start..].find('}') else { break };
        let ty = &text[start..start + len];
        for (id, _) in idents(ty) {
            add_leaf(root, ctx, &id, rootname, Kind::ImgType);
            images.push((id, true));
        }
        let after = start + len;
        let block_end = text[after..].find("</fo:instream-foreign-object").map_or(text.len(), |e| after + e);
        if let Some(q) = text[after..block_end].find("<?") {
            let s = after + q + 2;
            if let Some(e) = text[s..block_end].find("?>") {
                for (id, _) in idents(&text[s..s + e]) {
                    add_leaf(root, ctx, &id, rootname, Kind::ImgData);
                    images.push((id, false));
                }
            }
        }
        from = after;
    }
}

pub fn build(tags: &[String], docvars: &[String], rootname: &str, rows: usize) -> Sample {
    let mut root = E { name: rootname.to_string(), ..E::default() };
    let mut ctx: Vec<String> = Vec::new();
    let mut loop_stack: Vec<Vec<String>> = Vec::new();
    let mut saved: Vec<String> = Vec::new();
    let mut loops: Vec<String> = Vec::new();
    let mut images: Vec<(String, bool)> = Vec::new();

    for t in tags {
        let Some(inner) = t.strip_prefix("<?").and_then(|x| x.strip_suffix("?>")) else {
            scan_images(t, &mut root, &ctx, rootname, &mut images);
            continue;
        };
        let n = inner.trim();
        if let Some(rest) = n.strip_prefix("end ") {
            match rest.trim() {
                "for-each" | "for-each-group" => {
                    if let Some(p) = loop_stack.pop() {
                        ctx = p;
                    }
                }
                "template" => ctx = saved.clone(),
                _ => {}
            }
            continue;
        }
        let head = n.split(|c: char| c == ':' || c == '@' || c.is_whitespace()).next().unwrap_or("");
        match head {
            "for-each" | "for-each-group" => {
                let path = n.split_once(':').map_or("", |x| x.1).split(';').next().unwrap_or("").trim().to_string();
                loop_stack.push(ctx.clone());
                ctx = enter_loop(&mut root, &ctx, &path, rootname, rows);
                loops.push(path);
            }
            "template" => {
                saved = ctx.clone();
                ctx = Vec::new();
            }
            h if SKIP_HEADS.contains(&h) => {}
            _ => {
                for (id, num) in idents(n) {
                    add_leaf(&mut root, &ctx, &id, rootname, if num { Kind::Num } else { Kind::Text });
                }
            }
        }
    }
    for d in docvars {
        scan_images(d, &mut root, &[], rootname, &mut images);
    }

    let leaf = |p: &str| p.rsplit('/').next().unwrap_or(p).to_string();
    let names = Names {
        data: images.iter().filter(|x| !x.1).map(|x| leaf(&x.0)).collect(),
        types: images.iter().filter(|x| x.1).map(|x| leaf(&x.0)).collect(),
    };
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!-- sample data from bip-rtf: structure guessed from the template, values are made up -->\n");
    let mut fields = 0;
    write(&root, 0, &mut out, None, &names, &mut fields);
    let mut listed: Vec<String> = images.into_iter().map(|x| x.0).collect();
    listed.sort();
    listed.dedup();
    Sample { xml: out, fields, loops, images: listed }
}

struct Names {
    data: BTreeSet<String>,
    types: BTreeSet<String>,
}

fn value(e: &E, row: Option<usize>, names: &Names) -> String {
    let r = row.unwrap_or(1);
    let up = e.name.to_uppercase();
    let kind = e.leaf.unwrap_or(Kind::Text);
    if kind == Kind::ImgData || names.data.contains(&e.name) {
        return PNG.to_string();
    }
    if kind == Kind::ImgType || names.types.contains(&e.name) {
        return "image/png".to_string();
    }
    if kind == Kind::Num || ["AMOUNT", "PRICE", "TOTAL", "SUM", "QTY", "QUANTITY"].iter().any(|k| up.contains(k)) {
        return format!("{}.00", 100 * r);
    }
    if up.contains("DAT") {
        return "2026-11-05".to_string();
    }
    if ["TEL", "FAX", "PHONE"].iter().any(|k| up.contains(k)) {
        return format!("02123456{r:02}");
    }
    if row.is_some() { format!("{}_{r}", e.name) } else { format!("{}_SAMPLE", e.name) }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn write(e: &E, depth: usize, out: &mut String, row: Option<usize>, img: &Names, fields: &mut usize) {
    if e.repeat > 0 {
        for r in 1..=e.repeat {
            write_one(e, depth, out, Some(r), img, fields);
        }
    } else {
        write_one(e, depth, out, row, img, fields);
    }
}

fn write_one(e: &E, depth: usize, out: &mut String, row: Option<usize>, img: &Names, fields: &mut usize) {
    let pad = "  ".repeat(depth);
    if e.kids.is_empty() {
        *fields += 1;
        out.push_str(&format!("{pad}<{0}>{1}</{0}>\n", e.name, esc(&value(e, row, img))));
    } else {
        out.push_str(&format!("{pad}<{}>\n", e.name));
        for k in &e.kids {
            write(k, depth + 1, out, row, img, fields);
        }
        out.push_str(&format!("{pad}</{}>\n", e.name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(s: &str) -> Vec<String> {
        idents(s).into_iter().map(|x| x.0).collect()
    }

    #[test]
    fn expression_idents() {
        assert_eq!(names("if://DOC/PICTURE!=\"\""), ["//DOC/PICTURE"]);
        assert_eq!(names("xdofx: if (count(LINES) = 1) then '1' else '1-' || count(LINES) "), ["LINES", "LINES"]);
        assert_eq!(names("format-number(number(AMT1), '#,##0.00')"), ["AMT1"]);
        assert_eq!(names("position()"), Vec::<String>::new());
        let n = idents("sum(AMT1)");
        assert_eq!(n, [("AMT1".to_string(), true)]);
        assert_eq!(names("xdoxslt:toCheckNumber('en-US',GRAND_A, GRAND_B,'CASE_UPPER')"), ["GRAND_A", "GRAND_B"]);
    }

    #[test]
    fn loops_fields_and_images() {
        let tags: Vec<String> = [
            "<?CUSTOMER?>",
            "<?for-each:LINES?>",
            "<?QTY?>",
            "<?end for-each?>",
            "<?count(LINES)?>",
            "<?if://DOC/PICTURE!=\"\"?>",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let docvars = vec!["<?template:logo?><fo:instream-foreign-object content-type=\"{//DOC/PIC_TYPE}\"><?PIC_DATA?></fo:instream-foreign-object><?end template?>".to_string()];
        let s = build(&tags, &docvars, "DATA", 2);
        assert!(s.xml.contains("<CUSTOMER>CUSTOMER_SAMPLE</CUSTOMER>"), "{}", s.xml);
        assert_eq!(s.xml.matches("<LINES>").count(), 2, "{}", s.xml);
        assert!(s.xml.contains("<QTY>100.00</QTY>") && s.xml.contains("<QTY>200.00</QTY>"), "{}", s.xml);
        assert!(s.xml.contains("<DOC>") && s.xml.contains("<PIC_TYPE>image/png</PIC_TYPE>"), "{}", s.xml);
        assert!(s.xml.contains(&format!("<PIC_DATA>{PNG}</PIC_DATA>")), "{}", s.xml);
        assert_eq!(s.loops, ["LINES"]);
    }
}
