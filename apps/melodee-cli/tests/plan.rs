//! CLI-level regression coverage for the operator-facing roadmap status.

#![allow(clippy::expect_used)]

use std::process::Command;

#[test]
fn plan_reports_phase_five_complete_and_phase_six_active() {
    let output = Command::new(env!("CARGO_BIN_EXE_melodee-cli"))
        .arg("plan")
        .output()
        .expect("run melodee plan");
    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).expect("plan output is UTF-8");
    assert!(stdout.contains("Phase 3: complete — see design/evidence/phase-3/PHASE-3-REPORT.md"));
    assert!(stdout.contains("Phase 4: complete — see design/evidence/phase-4/PHASE-4-REPORT.md"));
    assert!(stdout.contains("Phase 5: complete — see design/evidence/phase-5/PHASE-5-REPORT.md"));
    assert!(stdout.contains("Phase 6: active — see IMPLEMENTATION.md"));
    assert!(!stdout.contains("Phase 5: active"));
    assert!(!stdout.contains("3.3 active"));
    assert!(!stdout.contains("3.4 planned"));
}
