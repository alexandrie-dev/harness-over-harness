//! String-only POSIX path operations matching Node's `path.posix`.

/// Normalize dot segments and repeated slashes, retaining a trailing slash.
pub fn normalize(path: &str) -> String {
    let absolute = is_absolute(path);
    let mut segments = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." if segments.last().is_some_and(|last| *last != "..") => {
                segments.pop();
            }
            ".." if absolute => {}
            _ => segments.push(segment),
        }
    }
    let mut result = segments.join("/");
    if absolute {
        result.insert(0, '/');
    } else if result.is_empty() {
        result.push('.');
    }
    if path.ends_with('/') && !result.ends_with('/') {
        result.push('/');
    }
    result
}

/// Join nonempty parts with slashes and normalize the result.
pub fn join(parts: &[&str]) -> String {
    normalize(
        &parts
            .iter()
            .copied()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// Resolve parts from right to left; absolute `cwd` replaces `process.cwd()`.
pub fn resolve(cwd: &str, parts: &[&str]) -> String {
    let start = parts.iter().rposition(|part| is_absolute(part));
    let joined = match start {
        Some(index) => parts[index..].join("/"),
        None => format!("{cwd}/{}", parts.join("/")),
    };
    let normalized = normalize(&joined);
    let trimmed = normalized.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// The relative path between two paths resolved against absolute `cwd`.
pub fn relative(cwd: &str, from: &str, to: &str) -> String {
    let from = resolve(cwd, &[from]);
    let to = resolve(cwd, &[to]);
    let from: Vec<_> = from.split('/').filter(|part| !part.is_empty()).collect();
    let to: Vec<_> = to.split('/').filter(|part| !part.is_empty()).collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts = vec![".."; from.len() - common];
    parts.extend_from_slice(&to[common..]);
    parts.join("/")
}

/// Return the directory spelling without normalizing dots or interior slashes.
pub fn dirname(path: &str) -> String {
    let end = path.trim_end_matches('/').len();
    if let Some(index) = path[..end].rfind('/') {
        if index == 0 {
            "/".to_owned()
        } else if index == 1 && is_absolute(path) {
            "//".to_owned()
        } else {
            path[..index].to_owned()
        }
    } else if is_absolute(path) {
        "/".to_owned()
    } else {
        ".".to_owned()
    }
}

/// Return the last nonempty slash-delimited part, without resolving dots.
pub fn basename(path: &str) -> String {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_owned()
}

/// Return the last dot suffix, excluding a sole leading dot and `..`.
pub fn extname(path: &str) -> String {
    let name = basename(path);
    match name.rfind('.') {
        Some(index) if index != 0 && name != ".." => name[index..].to_owned(),
        _ => String::new(),
    }
}

/// Whether the path starts with `/`, independently of the host platform.
pub fn is_absolute(path: &str) -> bool {
    path.starts_with('/')
}
