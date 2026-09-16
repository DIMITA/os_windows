use std::path::PathBuf;
use wilai_core::Config;

fn example_config_path() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // crates/
    p.pop(); // wilai/
    p.push("config");
    p.push("wilai.toml.example");
    p
}

#[test]
fn example_config_parses() {
    let path = example_config_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    // The example uses only commented-out MCP blocks; strip commented
    // lines aren't required, toml already skips them. Parse into Config.
    let cfg: Config = toml::from_str(&text)
        .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));

    // Sanity: at least the two documented provider blocks come through.
    assert!(cfg.providers.contains_key("ollama"), "ollama provider missing");
    assert!(cfg.providers.contains_key("claude"), "claude provider missing");
    // Sensible defaults survive when [general] is empty-ish.
    assert!(!cfg.general.default_model.is_empty());
    assert!(cfg.general.confirm_timeout_s >= 1);
}

#[test]
fn defaults_are_sane() {
    let cfg = Config::default();
    assert_eq!(cfg.general.default_provider, "ollama");
    assert!(cfg.providers.contains_key("ollama"));
    assert!(cfg.mode.auto_detect);
    assert!(cfg.mcp.is_empty());
}
