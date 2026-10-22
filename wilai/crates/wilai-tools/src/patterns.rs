use anyhow::{bail, Result};
use std::net::IpAddr;
use std::path::{Path, PathBuf};

pub fn validate(pattern: &str, value: &str) -> Result<()> {
    match pattern {
        "path" => check_path(value),
        "path_in_home" => check_path_in_home(value),
        "cidr" => check_cidr(value),
        "host" => check_host(value),
        "cidr_or_host" => check_cidr(value).or_else(|_| check_host(value)),
        "url" => check_url(value),
        "git_ref" => check_git_ref(value),
        "pkg_name" => check_pkg_name(value),
        "username" => check_username(value),
        "nmap_port_spec" => check_nmap_port_spec(value),
        // Anything else: treat as a raw regex. Covers the many YAML
        // tool specs that write `pattern: "^[a-zA-Z0-9…]+$"` inline.
        // Compile fresh each call — the catalogue is small and this
        // isn't on a hot path. An uncompilable regex falls back to a
        // "no such named pattern" error so YAML typos still surface.
        other => check_regex(other, value),
    }
}

fn check_regex(pattern: &str, value: &str) -> Result<()> {
    // An empty value against ANY regex that requires >=1 char would
    // fail; but YAML specs mark these fields `required: false`, and
    // the caller still ends up passing "" for optional inputs. Treat
    // empty as a no-op match — the executor's cmd template drops it.
    if value.is_empty() { return Ok(()); }
    let re = regex::Regex::new(pattern)
        .map_err(|e| anyhow::anyhow!("invalid pattern (not a known name and not a valid regex): {e}"))?;
    if !re.is_match(value) {
        bail!("value does not match pattern {pattern}");
    }
    Ok(())
}

fn check_path(v: &str) -> Result<()> {
    if v.is_empty() {
        bail!("empty path");
    }
    if v.contains('\0') {
        bail!("path contains NUL");
    }
    if v.contains('\n') {
        bail!("path contains newline");
    }
    Ok(())
}

fn check_path_in_home(v: &str) -> Result<()> {
    check_path(v)?;
    let home = wilai_core::paths::home_dir().ok_or_else(|| anyhow::anyhow!("no home"))?;
    let p = expand_tilde(v);
    let abs: PathBuf = if p.is_absolute() { p } else { std::env::current_dir()?.join(p) };
    let canon = std::fs::canonicalize(&abs).unwrap_or(abs);
    let home_canon = std::fs::canonicalize(&home).unwrap_or(home);
    if !canon.starts_with(&home_canon) {
        bail!("{} is outside $HOME", canon.display());
    }
    Ok(())
}

fn check_cidr(v: &str) -> Result<()> {
    let (host, prefix) = match v.rsplit_once('/') {
        Some((h, p)) => (h, Some(p)),
        None => (v, None),
    };
    let ip: IpAddr = host.parse().map_err(|e| anyhow::anyhow!("not an IP: {e}"))?;
    if let Some(p) = prefix {
        let p: u8 = p.parse().map_err(|_| anyhow::anyhow!("bad CIDR prefix"))?;
        let max = if ip.is_ipv4() { 32 } else { 128 };
        if p > max {
            bail!("CIDR prefix too large");
        }
    }
    Ok(())
}

fn check_host(v: &str) -> Result<()> {
    if v.is_empty() {
        bail!("empty host");
    }
    if v.parse::<IpAddr>().is_ok() {
        return Ok(());
    }
    let ok = v.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    });
    if !ok {
        bail!("not a valid DNS name: {v}");
    }
    Ok(())
}

fn check_url(v: &str) -> Result<()> {
    let lower = v.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        bail!("URL must be http(s)");
    }
    if v.contains(' ') || v.contains('\n') {
        bail!("URL contains whitespace");
    }
    Ok(())
}

fn check_git_ref(v: &str) -> Result<()> {
    if v.is_empty()
        || v.starts_with('-')
        || v.contains("..")
        || v.contains(' ')
        || v.contains('\n')
        || v.contains('~')
        || v.contains('^')
        || v.contains(':')
        || v.contains('?')
        || v.contains('*')
        || v.contains('[')
        || v.ends_with('/')
        || v.ends_with('.')
    {
        bail!("invalid git ref");
    }
    Ok(())
}

fn check_pkg_name(v: &str) -> Result<()> {
    let ok = !v.is_empty()
        && v.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.' | '+'));
    if !ok {
        bail!("invalid pkg name");
    }
    Ok(())
}

fn check_username(v: &str) -> Result<()> {
    if v.is_empty() || v.len() > 32 {
        bail!("username length");
    }
    let mut chars = v.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_lowercase() || first == '_') {
        bail!("username starts with bad char");
    }
    for c in chars {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-') {
            bail!("invalid char in username");
        }
    }
    Ok(())
}

fn check_nmap_port_spec(v: &str) -> Result<()> {
    if v.is_empty() {
        bail!("empty port spec");
    }
    if v == "top-1000" {
        return Ok(());
    }
    for part in v.split(',') {
        let part = part.trim_start_matches(|c: char| c == 'T' || c == 'U').trim_start_matches(':');
        if let Some((a, b)) = part.split_once('-') {
            let _: u16 = a.parse().map_err(|_| anyhow::anyhow!("bad port {a}"))?;
            let _: u16 = b.parse().map_err(|_| anyhow::anyhow!("bad port {b}"))?;
        } else {
            let _: u16 = part.parse().map_err(|_| anyhow::anyhow!("bad port {part}"))?;
        }
    }
    Ok(())
}

fn expand_tilde(s: &str) -> PathBuf {
    if let Some(stripped) = s.strip_prefix("~/") {
        if let Some(home) = wilai_core::paths::home_dir() {
            return home.join(stripped);
        }
    }
    PathBuf::from(s)
}

pub fn path_under(path: &Path, roots: &[&Path]) -> bool {
    let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    roots.iter().any(|r| {
        let rcanon = std::fs::canonicalize(r).unwrap_or_else(|_| (*r).to_path_buf());
        canon.starts_with(&rcanon)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_patterns_still_work() {
        assert!(validate("url", "https://example.com").is_ok());
        assert!(validate("cidr", "10.0.0.0/24").is_ok());
        assert!(validate("host", "example.com").is_ok());
        assert!(validate("path", "/etc/passwd").is_ok());
    }

    #[test]
    fn raw_regex_pattern_is_accepted() {
        // The regression bug: sys.service.list ships
        // pattern: "^[a-zA-Z0-9@._*?\\-]+$" as a raw regex. Validator
        // used to reject every non-named pattern.
        assert!(validate("^[a-zA-Z0-9@._*?\\-]+$", "ollama.service").is_ok());
        assert!(validate("^[0-9]+$", "42").is_ok());
    }

    #[test]
    fn raw_regex_mismatch_fails() {
        assert!(validate("^[0-9]+$", "abc").is_err());
        assert!(validate("^foo$", "bar").is_err());
    }

    #[test]
    fn empty_value_against_raw_regex_is_noop() {
        // Optional-input fields often arrive as "". A `+`-anchored
        // regex would reject that, but we treat empty as "not
        // supplied" rather than "invalid".
        assert!(validate("^[a-z]+$", "").is_ok());
    }

    #[test]
    fn invalid_regex_surfaces_as_error() {
        // Catch YAML typos — unclosed bracket class.
        let err = validate("^[a-z", "foo").unwrap_err().to_string();
        assert!(err.contains("not a valid regex"), "got: {err}");
    }
}
