//! Run the BI Publisher Template Builder engine (`RTF2PDFv2.jar`) on the command line.
//!
//! Nothing is guessed about the machine: the Java executable and the Template Builder folder come
//! from options or environment variables, and a missing one is reported with what to set.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct Opts {
    pub java: Option<String>,
    pub tb: Option<String>,
    pub locale: String,
    pub config: Option<String>,
    pub keep: bool,
    /// Java heap size in MB.
    pub memory_mb: u32,
    /// Seconds to wait for the engine before killing it.
    pub timeout_s: u64,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn abs(p: &str) -> Result<PathBuf, String> {
    std::fs::canonicalize(p).map_err(|e| format!("{p}: {e}")).map(|c| {
        let s = c.to_string_lossy().to_string();
        PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s)) // drop the Windows verbatim prefix
    })
}

pub fn render(rtf: &str, xml: &str, out: &str, o: &Opts) -> Result<String, String> {
    let tb = o
        .tb
        .clone()
        .or_else(|| std::env::var("BIPRTF_TB_HOME").ok())
        .ok_or("Template Builder folder unknown: pass --tb DIR or set BIPRTF_TB_HOME (the folder that contains jlib/RTF2PDFv2.jar)")?;
    let jar = Path::new(&tb).join("jlib").join("RTF2PDFv2.jar");
    if !jar.is_file() {
        return Err(format!("{} not found; check --tb / BIPRTF_TB_HOME", jar.display()));
    }
    let java = o.java.clone().or_else(|| std::env::var("BIPRTF_JAVA").ok()).unwrap_or_else(|| "java".to_string());
    let (rtf_abs, xml_abs) = (abs(rtf)?, abs(xml)?);
    if let Ok(oa) = abs(out)
        && (oa == rtf_abs || oa == xml_abs)
    {
        return Err("-o must not be the template or the data file".into());
    }

    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let work = std::env::temp_dir().join(format!("bip-rtf-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(work.join("tmp")).map_err(|e| format!("{}: {e}", work.display()))?;

    let mut p = String::from("<tb>\n  <type>PDF</type>\n");
    p.push_str(&format!("  <template>{}</template>\n  <data>{}</data>\n", esc(&rtf_abs.to_string_lossy()), esc(&xml_abs.to_string_lossy())));
    p.push_str(&format!("  <tmppath>{}</tmppath>\n  <locale>{}</locale>\n  <tbhome>{}</tbhome>\n", esc(&work.to_string_lossy()), esc(&o.locale), esc(&tb)));
    // The engine fails with a NullPointerException when any of these elements is missing.
    let cfg = match &o.config {
        Some(c) => c.clone(),
        None => {
            let d = work.join("config");
            std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
            d.to_string_lossy().to_string()
        }
    };
    p.push_str("  <xlifffile>\n  </xlifffile>\n  <mastertemplate>\n  </mastertemplate>\n");
    p.push_str(&format!("  <userconfigdir>{}</userconfigdir>\n", esc(&cfg)));
    p.push_str("</tb>\n");
    let params = work.join("params.xml");
    std::fs::write(&params, p).map_err(|e| e.to_string())?;

    let mut child = Command::new(&java)
        .arg(format!("-Xmx{}M", o.memory_mb))
        .arg("-jar")
        .arg(&jar)
        .arg(&params)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start `{java}`: {e}; pass --java PATH or set BIPRTF_JAVA"))?;
    let t0 = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break s;
        }
        if t0.elapsed() > Duration::from_secs(o.timeout_s) {
            let _ = child.kill();
            return Err(format!("engine did not finish within {} s; killed (raise --timeout)", o.timeout_s));
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    let log = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));

    let pdf = std::fs::read_dir(work.join("tmp"))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "pdf"));
    let result = match pdf {
        Some(p) if status.success() => {
            std::fs::copy(&p, out).map_err(|e| format!("{out}: {e}"))?;
            let size = std::fs::metadata(out).map_or(0, |m| m.len());
            Ok(format!(
                "{out}: {size} bytes. The engine can exit 0 and still write blank pages; check the content, for example with `pdfcraft-cli text`.{}",
                if log.trim().is_empty() { String::new() } else { format!("\nengine log:\n{}", log.trim()) }
            ))
        }
        _ => Err(format!("engine failed (exit {:?}), no PDF written\n{}", status.code(), log.trim())),
    };
    if !o.keep {
        let _ = std::fs::remove_dir_all(&work);
    } else {
        eprintln!("kept {}", work.display());
    }
    result
}
