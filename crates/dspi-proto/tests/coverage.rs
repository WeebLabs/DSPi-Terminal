//! The coverage guarantee.
//!
//! Every vendor opcode the firmware defines must be either referenced by a
//! registry row or listed in `EXCLUDED` with a written reason. This is what
//! turns "controls every setting" from a claim into something the build checks.
//!
//! When a firmware bump adds opcodes, this test names them. That is the intended
//! workflow: it is a worklist, not an obstacle. Never widen the assertion to
//! make it pass.

use dspi_proto::generated::ALL_OPCODES;
use dspi_proto::registry::{EXCLUDED, REGISTRY, referenced_opcodes};

#[test]
fn every_firmware_opcode_is_accounted_for() {
    let referenced = referenced_opcodes();
    let excluded: Vec<u8> = EXCLUDED.iter().map(|(op, _)| *op).collect();

    let missing: Vec<&str> = ALL_OPCODES
        .iter()
        .filter(|(_, code)| !referenced.contains(code) && !excluded.contains(code))
        .map(|(name, _)| *name)
        .collect();

    assert!(
        missing.is_empty(),
        "{} of {} firmware opcodes are unreachable from the registry.\n\
         Add a row for each, or add it to EXCLUDED with a reason:\n  {}",
        missing.len(),
        ALL_OPCODES.len(),
        missing.join("\n  ")
    );
}

/// An opcode cannot be both used and excluded; that means someone added a row
/// and forgot to remove the exclusion, leaving a stale and misleading reason.
#[test]
fn exclusions_are_not_also_registered() {
    let referenced = referenced_opcodes();
    for (code, reason) in EXCLUDED {
        assert!(
            !referenced.contains(code),
            "0x{code:02X} is registered but also excluded as \"{reason}\"; drop the exclusion"
        );
    }
}

#[test]
fn exclusions_carry_a_real_reason() {
    for (code, reason) in EXCLUDED {
        assert!(
            reason.len() > 10,
            "0x{code:02X} needs a proper explanation, not \"{reason}\""
        );
    }
}

/// Every opcode a row names must actually exist in the firmware. This catches a
/// row still referencing an opcode that a firmware bump removed.
#[test]
fn registry_references_no_phantom_opcodes() {
    let known: Vec<u8> = ALL_OPCODES.iter().map(|(_, c)| *c).collect();
    for d in REGISTRY {
        for (kind, code) in [("set", d.set), ("get", d.get)] {
            if let Some(code) = code {
                assert!(
                    known.contains(&code),
                    "{} names a {kind} opcode 0x{code:02X} that this firmware does not define",
                    d.path
                );
            }
        }
    }
}

/// Report coverage, so the number is visible rather than assumed.
#[test]
fn report_coverage() {
    let referenced = referenced_opcodes();
    let pct = 100.0 * referenced.len() as f64 / ALL_OPCODES.len() as f64;
    println!(
        "registry: {} rows referencing {}/{} opcodes ({pct:.0}%), {} excluded",
        REGISTRY.len(),
        referenced.len(),
        ALL_OPCODES.len(),
        EXCLUDED.len()
    );
}
