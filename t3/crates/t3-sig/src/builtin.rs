//! The built-in rule set, embedded as text.
//!
//! Keeping the rules as text rather than as Rust structures means the shipped set
//! goes through exactly the same parser as a user's file, so a rule that only works
//! because it was hand-constructed cannot exist. It also makes the set auditable in
//! one screen.
//!
//! # How these were chosen
//!
//! Every rule here targets a byte sequence that is *specific*: a tool banner, an
//! evasion artefact, or a shellcode idiom. Nothing here is a heuristic proxy for
//! "looks bad". That matters because a low-precision rule inside a signature engine
//! is worse than no rule — it costs a `Suspicious` on innocent files and teaches the
//! operator to ignore the output.
//!
//! Severities are deliberately modest. `high` is reserved for tool fingerprints that
//! have no benign reading (`sekurlsa::`, `metsrv`). Packer banners, evasion strings
//! and shellcode idioms are `medium` at most, because each of them does occur in
//! legitimate software. Environment strings are `low`: they are reported, and they
//! never move the grade.

/// The embedded rule text.
///
/// `t3 sig list` prints this verbatim, and `t3 sig check` re-parses it at start-up as
/// a self-test.
pub const BUILTIN_RULES: &str = r"
# ---------------------------------------------------------------------------
# Tool fingerprints. No benign reading: these strings exist to drive an attack.
# ---------------------------------------------------------------------------

rule mimikatz_sekurlsa
  severity high
  scope any
  desc Mimikatz credential module command prefix
  match 73 65 6B 75 72 6C 73 61 3A 3A
end

rule mimikatz_lsadump
  severity high
  scope any
  desc Mimikatz LSA dump command prefix
  match 6C 73 61 64 75 6D 70 3A 3A
end

rule meterpreter_server
  severity high
  scope any
  desc Meterpreter metsrv stage marker
  match 6D 65 74 73 72 76
end

# ---------------------------------------------------------------------------
# Packers. A banner is strong evidence of packing; packing is not itself
# malicious, hence medium rather than high.
# ---------------------------------------------------------------------------

rule upx_banner
  severity medium
  scope any
  desc UPX! magic, present in UPX-packed images
  match 55 50 58 21
end

rule aspack_banner
  severity medium
  scope any
  desc ASPack signature
  match 61 53 50 61 63 6B
end

# ---------------------------------------------------------------------------
# Evasion artefacts.
# ---------------------------------------------------------------------------

rule sandboxie_dll
  severity medium
  scope any
  desc Sandboxie library name, used for sandbox detection
  match 53 62 69 65 44 6C 6C 2E 64 6C 6C
end

rule shellcode_stack_align
  severity medium
  scope any
  desc cld; and rsp,-0x10 -- canonical shellcode prologue, rare in compiled code
  match FC 48 83 E4 F0
end

rule encoded_powershell
  severity medium
  scope any
  desc Base64-encoded PowerShell invocation switch
  match 2D 45 6E 63 6F 64 65 64 43 6F 6D 6D 61 6E 64
end

# ---------------------------------------------------------------------------
# Environment fingerprints. Informational only: these appear in real installers,
# virtualisation-aware applications and management agents.
# ---------------------------------------------------------------------------

rule vmware_guest_artefact
  severity low
  scope any
  desc VMware product string
  match 56 4D 77 61 72 65
end

rule virtualbox_guest_artefact
  severity low
  scope any
  desc VirtualBox guest service name
  match 56 42 6F 78 53 65 72 76 69 63 65
end

rule qemu_artefact
  severity low
  scope any
  desc QEMU virtual disk model string
  match 51 45 4D 55 20 48 41 52 44 44 49 53 4B
end

rule run_key
  severity low
  scope any
  desc Run-key registry path, the most common autostart location
  match 43 75 72 72 65 6E 74 56 65 72 73 69 6F 6E 5C 52 75 6E
end
";

#[cfg(test)]
mod tests {
    use super::BUILTIN_RULES;
    use crate::rule::{RuleSet, Severity};

    #[test]
    fn builtin_rules_parse() {
        let set = RuleSet::parse(BUILTIN_RULES).expect("builtin rules must always parse");
        assert!(set.len() >= 10, "only {} rules", set.len());
    }

    #[test]
    fn every_builtin_rule_has_a_description() {
        let set = RuleSet::parse(BUILTIN_RULES).unwrap();
        for rule in set.rules() {
            assert!(
                !rule.description.is_empty(),
                "rule {} has no desc",
                rule.name
            );
        }
    }

    #[test]
    fn high_severity_is_reserved_for_tool_fingerprints() {
        // Guards the precision claim in the module docs: a packer banner or an
        // environment string must never be promoted to high.
        let set = RuleSet::parse(BUILTIN_RULES).unwrap();
        for rule in set.rules() {
            if rule.severity == Severity::High {
                assert!(
                    rule.name.starts_with("mimikatz") || rule.name.starts_with("meterpreter"),
                    "rule {} claims high severity without a tool fingerprint",
                    rule.name
                );
            }
        }
    }

    #[test]
    fn no_rule_matches_only_wildcards() {
        let set = RuleSet::parse(BUILTIN_RULES).unwrap();
        for rule in set.rules() {
            for pattern in rule.matches.iter().chain(rule.forbids.iter()) {
                assert!(pattern.len() >= 2, "{}: pattern too short", rule.name);
            }
        }
    }
}
