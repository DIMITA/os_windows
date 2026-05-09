use serde_json::json;
use wilai_core::Mode;
use wilai_tools::guards::{evaluate, GuardOutcome};
use wilai_tools::spec::ToolSpec;

fn parse_spec(yaml: &str) -> ToolSpec {
    serde_yaml::from_str(yaml).unwrap()
}

#[test]
fn allow_when_no_guards_and_low_risk() {
    let spec = parse_spec(
        r#"
name: t.read
version: 1
description: read
category: read
risk: none
inputs: {}
executor:
  type: builtin
  fn: nope
  timeout_s: 1
  output: { capture: stdout, max_bytes: 1024 }
"#,
    );
    let out = evaluate(&spec, &json!({}), Mode::Normal).unwrap();
    assert!(matches!(out, GuardOutcome::Allow));
}

#[test]
fn confirm_default_for_medium_risk() {
    let spec = parse_spec(
        r#"
name: t.write
version: 1
description: write
category: write
risk: medium
inputs: {}
executor:
  type: builtin
  fn: nope
  timeout_s: 1
  output: { capture: stdout, max_bytes: 1024 }
"#,
    );
    let out = evaluate(&spec, &json!({}), Mode::Normal).unwrap();
    assert!(matches!(out, GuardOutcome::Confirm { .. }));
}

#[test]
fn explicit_confirm_guard_triggers() {
    let spec = parse_spec(
        r#"
name: t.scan
version: 1
description: scan
category: network
risk: low
inputs:
  rate_pps:
    type: integer
    required: true
guards:
  - if: "arg_gt(rate_pps, 1000)"
    confirm: "Rate {rate_pps} pps. OK?"
executor:
  type: builtin
  fn: nope
  timeout_s: 1
  output: { capture: stdout, max_bytes: 1024 }
"#,
    );
    let allow = evaluate(&spec, &json!({"rate_pps": 100}), Mode::Normal).unwrap();
    assert!(matches!(allow, GuardOutcome::Allow));
    let confirm = evaluate(&spec, &json!({"rate_pps": 5000}), Mode::Normal).unwrap();
    match confirm {
        GuardOutcome::Confirm { prompt, .. } => assert!(prompt.contains("5000")),
        _ => panic!("expected Confirm"),
    }
}

#[test]
fn deny_guard_blocks() {
    let spec = parse_spec(
        r#"
name: t.x
version: 1
description: x
category: write
risk: low
inputs:
  flag:
    type: boolean
    required: true
guards:
  - if: "arg_true(flag)"
    deny: "flag must not be set"
executor:
  type: builtin
  fn: nope
  timeout_s: 1
  output: { capture: stdout, max_bytes: 1024 }
"#,
    );
    let out = evaluate(&spec, &json!({"flag": true}), Mode::Normal).unwrap();
    match out {
        GuardOutcome::Deny { reason, .. } => assert!(reason.contains("must not be set")),
        _ => panic!("expected Deny"),
    }
}

#[test]
fn mode_required_blocks_in_normal() {
    let spec = parse_spec(
        r#"
name: t.scan
version: 1
description: scan
category: pentest
risk: medium
mode_required: pentest
inputs: {}
executor:
  type: subprocess
  cmd: ["true"]
  timeout_s: 1
  output: { capture: stdout, max_bytes: 1024 }
"#,
    );
    let out = evaluate(&spec, &json!({}), Mode::Normal).unwrap();
    assert!(matches!(out, GuardOutcome::Deny { .. }));
    let out = evaluate(&spec, &json!({}), Mode::Pentest).unwrap();
    // mode_required satisfied, falls through to risk medium -> Confirm
    assert!(matches!(out, GuardOutcome::Confirm { .. }));
}
