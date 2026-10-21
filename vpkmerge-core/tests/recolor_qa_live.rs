//! Recolor QA over every pinned hero recipe against the live game.
//!
//! Gated on `DEADLOCK_PAK` pointing at `citadel/pak01_dir.vpk`; CI skips.
//!
//! ```text
//! DEADLOCK_PAK=~/.../Deadlock/game/citadel/pak01_dir.vpk \
//!   cargo test --release -p vpkmerge-core --test recolor_qa_live -- --nocapture
//! ```
//!
//! Every pinned hero must `pass`: every pinned path resolves, patches and
//! decodes, no pin bleeds or hits data/skin/UI cues, and every color carrier is
//! pinned or excluded with a reason in `recolor_qa::REVIEWED`. Heroes in
//! [`KNOWN_WARN`] may warn (open gaps waiting on engine work) but never fail.

use vpkmerge_core::recolor_qa::DEFAULT_PROBE_HUE;
use vpkmerge_core::{audit_heroes, QaStatus};

const KNOWN_WARN: &[&str] = &[];

#[test]
fn recolor_recipes_pass_qa() {
    let Ok(pak) = std::env::var("DEADLOCK_PAK") else {
        eprintln!("DEADLOCK_PAK not set; skipping live recolor QA");
        return;
    };
    let run = audit_heroes(&pak, None, &[], DEFAULT_PROBE_HUE).expect("recolor QA run");

    for h in &run.heroes {
        eprintln!(
            "{:<11} {:<5} {:>4.0}%  gaps {:>3}  violations {:>2}  integrity {:>2}",
            h.codename,
            format!("{:?}", h.status),
            h.coverage * 100.0,
            h.gaps.len(),
            h.violations.len(),
            h.integrity.len()
        );
    }

    let failing: Vec<String> = run
        .heroes
        .iter()
        .filter(|h| match h.status {
            QaStatus::Pass => false,
            QaStatus::Warn => !KNOWN_WARN.contains(&h.codename.as_str()),
            QaStatus::Fail => true,
        })
        .map(|h| {
            let findings: Vec<String> = h
                .integrity
                .iter()
                .chain(&h.violations)
                .chain(&h.gaps)
                .chain(&h.noops)
                .map(|f| format!("  {} {}: {}", f.kind, f.entry, f.detail))
                .chain(
                    h.stale_reviews
                        .iter()
                        .map(|r| format!("  stale review {r}")),
                )
                .collect();
            format!("{} ({:?})\n{}", h.codename, h.status, findings.join("\n"))
        })
        .collect();
    assert!(failing.is_empty(), "recolor QA:\n{}", failing.join("\n"));
}
