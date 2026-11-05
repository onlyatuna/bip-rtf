# bip-rtf

A small command-line tool to inspect and make small edits to RTF report templates built for Oracle BI Publisher (templates made in Word with the BI Publisher add-in), without breaking their tags.

> Not affiliated with, endorsed by, or supported by Oracle. "Oracle" and "BI Publisher" are trademarks of Oracle and/or its affiliates; they are used here only to say which template format this tool reads.

Status: early (0.1). Written for a few real templates; read the limits below before relying on it.

## Why

In these templates the tag text is not in the visible text. Word stores it Base64-encoded in `{\*\docvar ...}` entries, and each legacy form field points to one with `<?ref:xdoNNNN?>`. Other tags are typed into the text and Word may split them across runs. A general RTF or document converter does not know this: opening and re-saving such a file with one can lose the tags. `bip-rtf` reads the file as it is and, when it edits, changes only the bytes it has to, then checks the result.

## Commands

```
bip-rtf tags  FILE [--json]        list every tag in document order (form field or typed text)
bip-rtf fonts FILE                 fonts used by CJK text and by other text
bip-rtf lint  FILE [--xml SAMPLE]  check tags; exit 1 on errors
bip-rtf images FILE                list pictures: number, kind, pixel size, display size
bip-rtf sample FILE [-o OUT.xml] [--rows N] [--root NAME]
                                   sample data XML guessed from the template
bip-rtf render FILE --xml DATA.xml -o OUT.pdf [--tb DIR] [--java EXE] [--locale L] [--config DIR]
                                   [--memory MB] [--timeout SEC] [--keep]
                                   make a PDF with the Template Builder engine

bip-rtf set-font  FILE --from NAME --to NAME -o OUT
bip-rtf set-tag   FILE --from '<?OLD?>' --to '<?NEW?>' -o OUT
bip-rtf set-image FILE --image N --png NEW.png -o OUT [--stretch]
```

The three `set-*` commands never edit in place: `-o` must be a different file.

### lint

Errors: unbalanced `for-each`, `if`, `choose`, `when`, `template`; a form field whose `<?ref:...?>` has no matching docvar; an unterminated `<?`. Warnings: CJK text with no font set. Items to look at (not errors): a tag with a leading or trailing space inside `<? ?>`; docvars that no field refers to; tag names missing from a sample XML (`--xml`). The tool does not run the BI Publisher engine, so it cannot tell whether an expression is valid.

### set-font

Renames every font-table entry whose name matches `--from` (case-insensitive). All text that uses that entry changes, including non-CJK text. Entries that end up with the same name cannot be told apart again afterwards.

### set-tag

Renames one whole tag. It changes the Base64 docvar, tags typed as plain text (when in one piece), and, for a simple `<?NAME?>` field, the form field's label (default text, the UTF-16 label in the binary data field, and the visible text). A tag split across Word runs is not found.

### set-image

Replaces the data of one PNG picture. It updates pixel and display size (by default the new image keeps its aspect and fits inside the old display box), and drops the old `\bliptag` and `{\*\blipuid ...}`. Other pictures, including a WMF stored for the same drawing, are not changed. A picture that fills a drawing shape is displayed at the shape's size, so a different aspect ratio looks stretched (the command warns). Pictures that come from data (an `fo:instream-foreign-object` with a field) are not in the template.

### sample

Writes sample data XML from the template: loops (`for-each`), field names in tags and expressions, absolute paths such as `//DOC/PICTURE`, and image blocks (the `content-type` path gets `image/png`, the data field gets a 1x1 PNG). A bare field name is placed under the root element, which is a guess. Values are made up.

### render

Runs Oracle's Template Builder engine (`RTF2PDFv2.jar`), which you must have installed under your own license; this project does not include or download it. It needs the Template Builder folder (the one containing `jlib/RTF2PDFv2.jar`): `--tb DIR` or the `BIPRTF_TB_HOME` environment variable. It needs Java: `--java EXE`, `BIPRTF_JAVA`, or `java` on PATH. If either is missing the command says what to set. Defaults: 256 MB of Java heap (`--memory`), 180 seconds before the engine is killed (`--timeout`), locale `en-US` (`--locale`). Only PDF output is supported.

The engine can exit 0 and still write a PDF with blank pages (for example when the data lacks an element the template needs), and it may open the PDF in the default viewer. Check the content of the PDF, not only that it exists.

## How edits are checked

Before writing, each `set-*` command parses the new file and compares it with the original: the tag list must equal the original (with the rename applied for `set-tag`), and docvars, pictures, fonts, and text must be unchanged (for `set-image`, the new picture data must equal your PNG and the other pictures must be byte-identical). If anything differs, nothing is written. This is a structural check; it is not a substitute for opening the result in Word and in BI Publisher.

## Build

Needs a Rust toolchain with edition 2024 support (Rust 1.85 or newer).

```
cargo build --release
cargo test
```

## Limits and assumptions

What the tool assumes about your environment and templates. If one of these does not hold, results can be wrong or a command can refuse.

| Area | Assumption |
|---|---|
| Template format | RTF as written by Word with the BI Publisher add-in: tags in `\docvar` entries, legacy form fields (`fdeftext`, `\datafield`, `ldrslt`), docvar names like `xdo0001`. RTF from other programs is not supported. |
| Character sets | Font-name and text decoding knows the usual Windows charsets and code pages (Western, Central/Eastern European, Cyrillic, Greek, Turkish, Hebrew, Arabic, Baltic, Vietnamese, Thai, Simplified and Traditional Chinese, Japanese, Korean). Others are read as Windows-1252. |
| Tag names | `set-tag` accepts non-ASCII tags only where the tag text is in a docvar. A non-ASCII tag typed as plain text is not matched, so the edit is refused. The visible label of a field is updated only for simple ASCII names (`<?NAME?>`). |
| `sample` | Root element `DATA`, 3 rows, made-up values. Values are chosen from field names in English (names containing AMOUNT, PRICE, TOTAL, SUM, QTY get numbers; TEL, FAX, PHONE get phone-like text; DAT gets a date). Other naming just gets text values. |
| `set-image` | PNG only, up to 20 MB. A picture that fills a drawing shape is detected by the shape property `pib`. |
| `render` | The Template Builder folder has `jlib/RTF2PDFv2.jar` and takes the parameter file this tool writes. Tested with one Template Builder version. Java heap, timeout and locale are options; PDF is the only output. The engine may open the PDF in the default viewer, and may exit 0 while writing blank pages. |
| Terminal | Output is UTF-8. On a Windows console that is not UTF-8, the tool switches the console to UTF-8 while it runs and restores it afterwards. Other terminals must be UTF-8. |
| Platforms | Developed and tested on Windows with Word and the Template Builder engine, on a small number of real templates (none are included here). CI builds and tests on Linux, macOS and Windows, but only unit tests run there; the Word and engine checks have not been run on other platforms. |

Not supported: adding or deleting fields, changing loops, replacing WMF pictures, templates in formats other than RTF. If a result looks wrong, check the template in Word.

## License

Licensed under either of Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE)) or MIT license ([LICENSE-MIT](LICENSE-MIT)) at your option.
