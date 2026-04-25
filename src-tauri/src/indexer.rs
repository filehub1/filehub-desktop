use serde::Serialize;
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::SystemTime;
use walkdir::WalkDir;
use regex::Regex;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub modified: u64,
    pub is_directory: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStatus {
    pub status: String,
    pub file_count: usize,
    pub indexed_directories: Vec<String>,
}

#[derive(Clone)]
pub struct FileIndex {
    files: Arc<RwLock<Vec<FileEntry>>>,
    indexing: Arc<RwLock<bool>>,
    directories: Arc<RwLock<Vec<String>>>,
    exclude_patterns: Arc<RwLock<Vec<String>>>,
}

impl FileIndex {
    pub fn new(directories: Vec<String>, exclude_patterns: Vec<String>) -> Self {
        Self {
            files: Arc::new(RwLock::new(vec![])),
            indexing: Arc::new(RwLock::new(false)),
            directories: Arc::new(RwLock::new(directories)),
            exclude_patterns: Arc::new(RwLock::new(exclude_patterns)),
        }
    }

    pub fn update_config(&self, directories: Vec<String>, exclude_patterns: Vec<String>) {
        *self.directories.write().unwrap() = directories;
        *self.exclude_patterns.write().unwrap() = exclude_patterns;
    }

    pub fn rebuild(&self) {
        if *self.indexing.read().unwrap() {
            return;
        }
        *self.indexing.write().unwrap() = true;

        let files_lock = self.files.clone();
        let indexing_lock = self.indexing.clone();
        let dirs = self.directories.read().unwrap().clone();
        let patterns = self.exclude_patterns.read().unwrap().clone();

        std::thread::spawn(move || {
            let matchers: Vec<Regex> = patterns
                .iter()
                .filter_map(|p| glob_to_regex(p).ok())
                .collect();

            let mut new_files = Vec::new();
            for dir in &dirs {
                for entry in WalkDir::new(dir).follow_links(false).into_iter().flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if matchers.iter().any(|r| r.is_match(&name)) {
                        if entry.file_type().is_dir() {
                            // skip entire subtree by not descending — WalkDir handles this
                            // via filter_entry, but here we just skip the entry itself
                        }
                        continue;
                    }
                    let meta = match entry.metadata() {
                        Ok(m) => m,
                        Err(_) => continue,
                    };
                    let modified = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    new_files.push(FileEntry {
                        name,
                        path: entry.path().to_string_lossy().to_string(),
                        size: meta.len(),
                        modified,
                        is_directory: meta.is_dir(),
                    });
                }
            }

            tracing::info!("Index complete: {} files", new_files.len());
            *files_lock.write().unwrap() = new_files;
            *indexing_lock.write().unwrap() = false;
        });
    }

    pub fn search(
        &self,
        query: &str,
        max_results: usize,
        search_type: &str,
        search_in_path: bool,
    ) -> Vec<FileEntry> {
        if query.trim().is_empty() {
            return vec![];
        }
        let files = self.files.read().unwrap();
        let query_lower = query.trim().to_lowercase();

        let mut results: Vec<&FileEntry> = match search_type {
            "regex" => {
                let Ok(re) = Regex::new(&format!("(?i){}", query)) else { return vec![] };
                files
                    .iter()
                    .filter(|f| {
                        let target = if search_in_path { &f.path } else { &f.name };
                        re.is_match(target)
                    })
                    .collect()
            }
            "fuzzy" => files
                .iter()
                .filter(|f| {
                    let target = if search_in_path {
                        f.path.to_lowercase()
                    } else {
                        f.name.to_lowercase()
                    };
                    fuzzy_match(&target, &query_lower)
                })
                .collect(),
            _ => {
                let parts: Vec<&str> = query_lower.split_whitespace().collect();
                let mut matched: Vec<&FileEntry> = files
                    .iter()
                    .filter(|f| {
                        let target = if search_in_path {
                            f.path.to_lowercase()
                        } else {
                            f.name.to_lowercase()
                        };
                        parts.iter().all(|p| target.contains(p))
                    })
                    .collect();
                matched.sort_by(|a, b| {
                    let a_starts = a.name.to_lowercase().starts_with(&query_lower);
                    let b_starts = b.name.to_lowercase().starts_with(&query_lower);
                    b_starts.cmp(&a_starts)
                        .then(b.is_directory.cmp(&a.is_directory))
                        .then(a.name.len().cmp(&b.name.len()))
                });
                matched
            }
        };

        results.truncate(max_results);
        results.into_iter().cloned().collect()
    }

    pub fn status(&self) -> IndexStatus {
        IndexStatus {
            status: if *self.indexing.read().unwrap() {
                "indexing".to_string()
            } else {
                "idle".to_string()
            },
            file_count: self.files.read().unwrap().len(),
            indexed_directories: self.directories.read().unwrap().clone(),
        }
    }
}

fn glob_to_regex(pattern: &str) -> Result<Regex, regex::Error> {
    let mut re = String::from("(?i)^");
    for ch in pattern.chars() {
        match ch {
            '*' => re.push_str(".*"),
            '?' => re.push('.'),
            '.' | '+' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' | '\\' => {
                re.push('\\');
                re.push(ch);
            }
            c => re.push(c),
        }
    }
    re.push('$');
    Regex::new(&re)
}

fn fuzzy_match(text: &str, pattern: &str) -> bool {
    let mut chars = text.chars();
    pattern.chars().all(|p| chars.any(|c| c == p))
}
