use encoding_rs::Encoding;
use serde::Serialize;
use std::fs;
use std::io::Read;
use std::path::Path;

const MAX_TEXT_BYTES: usize = 256 * 1024;
const MAX_BINARY_BYTES: u64 = 50 * 1024 * 1024;

static TEXT_EXTS: &[&str] = &[
    "txt", "text", "md", "mdx", "rst", "adoc", "tex",
    "json", "jsonc", "json5", "js", "mjs", "cjs", "ts", "mts", "cts", "jsx", "tsx",
    "css", "scss", "sass", "less", "html", "htm", "xhtml", "xml",
    "yml", "yaml", "toml", "ini", "cfg", "conf", "config", "cnf",
    "log", "out", "err", "properties", "prop", "env",
    "sh", "bash", "zsh", "fish", "bat", "cmd", "ps1",
    "py", "pyw", "java", "c", "cc", "cpp", "cxx", "h", "hh", "hpp", "hxx",
    "cs", "fs", "vb", "go", "rs", "rb", "php", "swift", "kt", "kts", "scala",
    "sql", "csv", "tsv", "lua", "r", "graphql", "gql", "proto", "dart",
    "clj", "groovy", "gradle", "vue", "svelte", "lock",
];

static BINARY_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "webp", "ico", "svg",
    "mp4", "webm", "avi", "mkv", "mov", "wmv",
    "mp3", "wav", "ogg", "flac", "aac", "m4a",
    "pdf", "xlsx", "xls",
];

static TEXT_FILENAMES: &[&str] = &[
    "dockerfile", "makefile", "license", "licence", "readme",
    ".gitignore", ".gitattributes", ".editorconfig", ".npmrc",
    ".prettierrc", ".eslintrc", ".babelrc", ".nvmrc",
];

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ext: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_encoding: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileInfoResponse {
    pub size: u64,
    pub created: u64,
    pub modified: u64,
    pub accessed: u64,
    pub is_directory: bool,
    pub extension: String,
}

pub fn get_file_info(path: &str) -> Option<FileInfoResponse> {
    use std::time::SystemTime;
    let meta = fs::metadata(path).ok()?;
    let ts = |t: std::io::Result<SystemTime>| {
        t.ok()
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    };
    Some(FileInfoResponse {
        size: meta.len(),
        created: ts(meta.created()),
        modified: ts(meta.modified()),
        accessed: ts(meta.accessed()),
        is_directory: meta.is_dir(),
        extension: Path::new(path)
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
            .unwrap_or_default(),
    })
}

pub fn get_preview(path: &str) -> PreviewResponse {
    let p = Path::new(path);
    let ext = p
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let ext_dot = format!(".{}", ext);
    let filename = p
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) => return err(&ext_dot, &e.to_string()),
    };

    // Text
    if TEXT_EXTS.contains(&ext.as_str()) || TEXT_FILENAMES.iter().any(|n| *n == filename.as_str()) {
        return read_text(path, &ext_dot, meta.len());
    }

    // SVG — binary ext but text content
    if ext == "svg" {
        return read_text(path, &ext_dot, meta.len());
    }

    // docx / pptx
    if ext == "docx" || ext == "pptx" {
        return extract_openxml(path, &ext_dot);
    }

    // Binary preview
    if BINARY_EXTS.contains(&ext.as_str()) {
        if meta.len() > MAX_BINARY_BYTES {
            return err(&ext_dot, &format!("File too large ({} MB)", meta.len() / 1024 / 1024));
        }
        let data = match fs::read(path) {
            Ok(d) => d,
            Err(e) => return err(&ext_dot, &e.to_string()),
        };
        return PreviewResponse {
            success: true,
            data: Some(base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &data)),
            ext: Some(ext_dot),
            content_encoding: Some("base64".to_string()),
            error: None,
            truncated: None,
        };
    }

    // Unknown — try as text
    read_text(path, &ext_dot, meta.len())
}

fn read_text(path: &str, ext: &str, file_size: u64) -> PreviewResponse {
    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return err(ext, &e.to_string()),
    };
    let read_size = MAX_TEXT_BYTES.min(file_size as usize);
    let mut buf = vec![0u8; read_size];
    let n = match file.read(&mut buf) {
        Ok(n) => n,
        Err(e) => return err(ext, &e.to_string()),
    };
    buf.truncate(n);

    let text = decode_text(&buf);
    PreviewResponse {
        success: true,
        data: Some(text),
        ext: Some(ext.to_string()),
        content_encoding: Some("utf8".to_string()),
        truncated: Some(file_size > MAX_TEXT_BYTES as u64),
        error: None,
    }
}

fn decode_text(buf: &[u8]) -> String {
    // BOM detection
    if buf.len() >= 3 && buf[0] == 0xef && buf[1] == 0xbb && buf[2] == 0xbf {
        return String::from_utf8_lossy(&buf[3..]).to_string();
    }
    if buf.len() >= 2 && ((buf[0] == 0xff && buf[1] == 0xfe) || (buf[0] == 0xfe && buf[1] == 0xff)) {
        let (text, _, _) = encoding_rs::UTF_16LE.decode(buf);
        return text.replace('\0', "");
    }
    String::from_utf8_lossy(buf).to_string()
}

fn extract_openxml(path: &str, ext: &str) -> PreviewResponse {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return err(ext, &e.to_string()),
    };
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(a) => a,
        Err(e) => return err(ext, &e.to_string()),
    };

    let is_docx = ext == ".docx";
    let mut text_parts: Vec<String> = vec![];

    for i in 0..archive.len() {
        let mut entry = match archive.by_index(i) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = entry.name().to_string();
        let relevant = if is_docx {
            name == "word/document.xml"
        } else {
            name.starts_with("ppt/slides/slide") && name.ends_with(".xml")
        };
        if !relevant {
            continue;
        }
        let mut content = String::new();
        if entry.read_to_string(&mut content).is_ok() {
            text_parts.push(strip_xml(&content));
        }
        if is_docx {
            break;
        }
    }

    if text_parts.is_empty() {
        return err(ext, "No text content found");
    }

    PreviewResponse {
        success: true,
        data: Some(text_parts.join("\n\n")),
        ext: Some(ext.to_string()),
        content_encoding: Some("utf8".to_string()),
        error: None,
        truncated: None,
    }
}

fn strip_xml(xml: &str) -> String {
    let s = xml
        .replace("<w:tab/>", "\t")
        .replace("<w:br/>", "\n")
        .replace("<a:br/>", "\n");
    let re = regex::Regex::new(r"<[^>]+>").unwrap();
    let s = re.replace_all(&s, " ");
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn err(ext: &str, msg: &str) -> PreviewResponse {
    PreviewResponse {
        success: false,
        error: Some(msg.to_string()),
        ext: Some(ext.to_string()),
        data: None,
        content_encoding: None,
        truncated: None,
    }
}
