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
        other => bail!("unknown pattern: {other}"),
    }
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
