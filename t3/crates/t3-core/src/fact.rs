//! The observation table.
//!
//! Every structural thing the tool learns about a file is written here as a
//! [`Tri`], and every reader of this table is forced to consider that a value may
//! be `Unknown`. That is the whole point: a parser that cannot read a section
//! table says so, and the statement propagates into the verdict instead of
//! quietly becoming "not suspicious".
//!
//! # Why a fixed slot array
//!
//! Facts are a closed set known at compile time, so they live in a `[Cell; N]`
//! indexed by [`FactId`]. Scanning ten thousand files therefore performs zero
//! allocations and zero hashing for the evidence layer. The reference
//! implementation this tool was designed against uses `BTreeMap<String, _>` plus
//! a fresh `String` per lookup, which is a per-file heap cost on the hot path.
//!
//! The trade-off is real: adding a fact means editing one enum. That is the
//! correct trade for a tool whose entire job is a fixed, auditable checklist.

use crate::band::Band;
use crate::Tri;

/// One observation slot.
#[derive(Clone, Copy)]
pub struct Cell {
    /// Three-valued conclusion.
    pub val: Tri,
    /// The measurement behind it, when numeric. `f32::NAN` means "not numeric".
    pub num: f32,
    /// Static label for reports. Never allocated.
    pub note: &'static str,
}

impl Cell {
    /// An untouched slot: unknown, no measurement.
    pub const UNSET: Cell = Cell {
        val: Tri::Unknown,
        num: f32::NAN,
        note: "",
    };

    /// True when nothing was recorded.
    #[inline]
    #[must_use]
    pub fn is_unset(&self) -> bool {
        self.note.is_empty() && self.val.is_unknown()
    }
}

/// Identifier of an observation.
///
/// The discriminants are dense from zero and are used directly as array indices,
/// which a unit test enforces ([`tests::ids_are_dense_and_names_unique`]).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(u16)]
pub enum FactId {
    // ---- container -------------------------------------------------------
    /// Recognised as PE.
    FormatPe,
    /// Recognised as ELF.
    FormatElf,
    /// File size in bytes, at least `MIN_TRUSTED_SIZE`.
    SizeSubstantial,
    /// Whole-file Shannon entropy exceeded the packing band.
    FileHighEntropy,

    // ---- sections --------------------------------------------------------
    /// Highest entropy over **executable** sections: code that does not look like
    /// compiled code.
    SectionHighEntropy,
    /// Highest entropy over **non-executable** sections: an embedded blob.
    ///
    /// Kept separate from [`FactId::SectionHighEntropy`] because the two mean different
    /// things. A packed code section says the instructions were compressed or
    /// encrypted; a high-entropy data section says something bulk was appended. Merging
    /// them flagged every graphics driver on this machine as packed, since vendor
    /// drivers keep compiled GPU kernels in `.data`.
    SectionHighEntropyData,
    /// A section name matches a known packer/protector.
    SectionPackerName,
    /// Section table is structurally inconsistent (overlaps, out-of-file).
    SectionMalformed,
    /// A section is both writable and executable.
    SectionWritableExecutable,
    /// A section declares virtual size but carries no raw data (unpacking stub).
    SectionZeroRaw,
    /// Number of sections is anomalous (0, 1, or absurdly many).
    SectionCountAnomalous,

    // ---- entry point -----------------------------------------------------
    /// Entry point does not land inside an executable section.
    EntryOutsideCode,
    /// Entry point lands in the last section, a common packer layout.
    EntryInLastSection,

    // ---- overlay ---------------------------------------------------------
    /// Bytes exist past the last section's raw data.
    OverlayPresent,
    /// Overlay is large and high-entropy (embedded payload).
    OverlayHighEntropy,
    /// Overlay occupies a large fraction of the file.
    OverlayDominant,

    // ---- imports ---------------------------------------------------------
    /// Import table was parsed completely.
    ImportsReadable,
    /// Import directory is absent (normal for some Go/Rust and shellcode-ish).
    ImportsAbsent,
    /// Very few imports relative to size.
    ImportsSparse,
    /// Runtime loader APIs present (`LoadLibrary*`, `GetProcAddress`, `LdrLoadDll`).
    ImportsLoader,
    /// Network APIs present.
    ImportsNetwork,
    /// Credential/secret-access APIs present.
    ImportsCredential,
    /// Process injection trio: allocate-remote, write-remote, execute-remote.
    ImportsInjectionTrio,
    /// Anti-debugging APIs present.
    ImportsAntiDebug,
    /// Process/thread enumeration APIs present.
    ImportsProcessEnum,
    /// Cryptographic APIs present.
    ImportsCrypto,
    /// Service/persistence control APIs present.
    ImportsPersistence,
    /// Dynamic resolution without a matching static import: obfuscation signal.
    ImportsDynamicResolveOnly,

    // ---- provenance ------------------------------------------------------
    /// Authenticode signature directory is present and non-empty.
    SignedPresent,
    /// Debug directory carries a PDB path.
    PdbPresent,
    /// PDB path points at a developer profile (build-tree leakage).
    PdbDeveloperPath,
    /// Rich header present and key-valid, i.e. built by MSVC.
    RichHeaderValid,
    /// `.NET` CLR directory present.
    DotnetPresent,
    /// TLS callback directory present.
    TlsCallbacks,
    /// Relocation directory present.
    RelocationsPresent,
    /// Resource/version directory present.
    ResourcesPresent,

    // ---- anomalies -------------------------------------------------------
    /// PE timestamp is zero or in the future.
    TimestampAnomalous,
    /// Section table's raw offsets are not monotone.
    LayoutNonMonotone,
    /// ELF: position-independent executable.
    ElfPie,
    /// ELF: `GNU_STACK` is non-executable.
    ElfNxStack,
    /// ELF: build-id note present.
    ElfBuildId,
    /// ELF: symbol table stripped.
    ElfStripped,
    /// ELF: text segment is writable.
    ElfWritableText,

    // ---- signature engine ------------------------------------------------
    /// At least one high-severity signature matched.
    SigHigh,
    /// At least one medium-severity signature matched.
    SigMedium,
}

impl FactId {
    /// Total number of fact slots.
    pub const COUNT: usize = 45;

    /// Dense list, in discriminant order.
    pub const ALL: [FactId; Self::COUNT] = [
        FactId::FormatPe,
        FactId::FormatElf,
        FactId::SizeSubstantial,
        FactId::FileHighEntropy,
        FactId::SectionHighEntropy,
        FactId::SectionHighEntropyData,
        FactId::SectionPackerName,
        FactId::SectionMalformed,
        FactId::SectionWritableExecutable,
        FactId::SectionZeroRaw,
        FactId::SectionCountAnomalous,
        FactId::EntryOutsideCode,
        FactId::EntryInLastSection,
        FactId::OverlayPresent,
        FactId::OverlayHighEntropy,
        FactId::OverlayDominant,
        FactId::ImportsReadable,
        FactId::ImportsAbsent,
        FactId::ImportsSparse,
        FactId::ImportsLoader,
        FactId::ImportsNetwork,
        FactId::ImportsCredential,
        FactId::ImportsInjectionTrio,
        FactId::ImportsAntiDebug,
        FactId::ImportsProcessEnum,
        FactId::ImportsCrypto,
        FactId::ImportsPersistence,
        FactId::ImportsDynamicResolveOnly,
        FactId::SignedPresent,
        FactId::PdbPresent,
        FactId::PdbDeveloperPath,
        FactId::RichHeaderValid,
        FactId::DotnetPresent,
        FactId::TlsCallbacks,
        FactId::RelocationsPresent,
        FactId::ResourcesPresent,
        FactId::TimestampAnomalous,
        FactId::LayoutNonMonotone,
        FactId::ElfPie,
        FactId::ElfNxStack,
        FactId::ElfBuildId,
        FactId::ElfStripped,
        FactId::ElfWritableText,
        FactId::SigHigh,
        FactId::SigMedium,
    ];

    /// Stable machine-readable name. Part of the JSONL contract.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            FactId::FormatPe => "format.pe",
            FactId::FormatElf => "format.elf",
            FactId::SizeSubstantial => "size.substantial",
            FactId::FileHighEntropy => "file.high_entropy",
            FactId::SectionHighEntropy => "section.high_entropy",
            FactId::SectionHighEntropyData => "section.high_entropy_data",
            FactId::SectionPackerName => "section.packer_name",
            FactId::SectionMalformed => "section.malformed",
            FactId::SectionWritableExecutable => "section.writable_executable",
            FactId::SectionZeroRaw => "section.zero_raw",
            FactId::SectionCountAnomalous => "section.count_anomalous",
            FactId::EntryOutsideCode => "entry.outside_code",
            FactId::EntryInLastSection => "entry.in_last_section",
            FactId::OverlayPresent => "overlay.present",
            FactId::OverlayHighEntropy => "overlay.high_entropy",
            FactId::OverlayDominant => "overlay.dominant",
            FactId::ImportsReadable => "imports.readable",
            FactId::ImportsAbsent => "imports.absent",
            FactId::ImportsSparse => "imports.sparse",
            FactId::ImportsLoader => "imports.loader",
            FactId::ImportsNetwork => "imports.network",
            FactId::ImportsCredential => "imports.credential",
            FactId::ImportsInjectionTrio => "imports.injection_trio",
            FactId::ImportsAntiDebug => "imports.anti_debug",
            FactId::ImportsProcessEnum => "imports.process_enum",
            FactId::ImportsCrypto => "imports.crypto",
            FactId::ImportsPersistence => "imports.persistence",
            FactId::ImportsDynamicResolveOnly => "imports.dynamic_resolve_only",
            FactId::SignedPresent => "signed.present",
            FactId::PdbPresent => "pdb.present",
            FactId::PdbDeveloperPath => "pdb.developer_path",
            FactId::RichHeaderValid => "rich.valid",
            FactId::DotnetPresent => "dotnet.present",
            FactId::TlsCallbacks => "tls.callbacks",
            FactId::RelocationsPresent => "relocations.present",
            FactId::ResourcesPresent => "resources.present",
            FactId::TimestampAnomalous => "timestamp.anomalous",
            FactId::LayoutNonMonotone => "layout.non_monotone",
            FactId::ElfPie => "elf.pie",
            FactId::ElfNxStack => "elf.nx_stack",
            FactId::ElfBuildId => "elf.build_id",
            FactId::ElfStripped => "elf.stripped",
            FactId::ElfWritableText => "elf.writable_text",
            FactId::SigHigh => "sig.high",
            FactId::SigMedium => "sig.medium",
        }
    }

    /// Array index.
    #[inline]
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }
}

/// The observation table for one file.
///
/// `Default` is all-unknown, which is the correct starting point: a fact is only
/// decided once a detector had the evidence to decide it.
#[derive(Clone)]
pub struct Facts {
    cells: [Cell; FactId::COUNT],
}

impl Default for Facts {
    fn default() -> Self {
        Facts {
            cells: [Cell::UNSET; FactId::COUNT],
        }
    }
}

impl Facts {
    /// A table in which everything is unknown.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a decision with an optional measurement and static label.
    #[inline]
    pub fn set(&mut self, id: FactId, val: Tri, num: f32, note: &'static str) {
        self.cells[id.index()] = Cell { val, num, note };
    }

    /// Records a boolean decision.
    #[inline]
    pub fn set_bool(&mut self, id: FactId, val: bool, note: &'static str) {
        self.set(id, Tri::from_bool(val), f32::NAN, note);
    }

    /// Records a boolean decision together with the number that produced it.
    #[inline]
    pub fn set_bool_at(&mut self, id: FactId, val: bool, num: f32, note: &'static str) {
        self.set(id, Tri::from_bool(val), num, note);
    }

    /// Records `Unknown`, with the reason as the label.
    #[inline]
    pub fn set_unknown(&mut self, id: FactId, note: &'static str) {
        self.set(id, Tri::Unknown, f32::NAN, note);
    }

    /// Triangulates a continuous measurement against a calibrated band.
    ///
    /// This is the primitive that keeps graded evidence from being flattened into
    /// a bare comparison. `band.lo == band.hi` is a legacy hard threshold; any
    /// wider band reports `Unknown` inside itself, which is the honest answer
    /// when the measurement sits in the region where clean and packed corpora
    /// overlap.
    #[inline]
    pub fn set_band(&mut self, id: FactId, value: f32, band: Band, note: &'static str) {
        self.set(id, band.classify(value), value, note);
    }

    /// Reads a decision. Unset slots read as `Unknown`.
    #[inline]
    #[must_use]
    pub fn get(&self, id: FactId) -> Tri {
        self.cells[id.index()].val
    }

    /// Reads the whole cell.
    #[inline]
    #[must_use]
    pub fn cell(&self, id: FactId) -> &Cell {
        &self.cells[id.index()]
    }

    /// Reads a slot's measurement, if it has one.
    #[inline]
    #[must_use]
    pub fn num(&self, id: FactId) -> Option<f32> {
        let n = self.cells[id.index()].num;
        if n.is_nan() {
            None
        } else {
            Some(n)
        }
    }

    /// Number of decided (non-`Unknown`) slots.
    #[must_use]
    pub fn decided_count(&self) -> usize {
        self.cells.iter().filter(|c| !c.val.is_unknown()).count()
    }

    /// Iterates every cell with its identifier, in discriminant order.
    pub fn iter(&self) -> impl Iterator<Item = (FactId, &Cell)> + '_ {
        FactId::ALL
            .iter()
            .copied()
            .map(|id| (id, &self.cells[id.index()]))
    }
}

#[cfg(test)]
mod tests {
    use super::{FactId, Facts};
    use crate::band::Band;
    use crate::Tri;

    #[test]
    fn ids_are_dense_and_names_unique() {
        assert_eq!(FactId::ALL.len(), FactId::COUNT);
        let mut names = std::collections::BTreeSet::new();
        for (i, id) in FactId::ALL.iter().enumerate() {
            assert_eq!(id.index(), i, "discriminant {id:?} is not dense");
            assert!(names.insert(id.name()), "duplicate name {}", id.name());
            assert_ne!(id.name(), "");
        }
    }

    #[test]
    fn default_is_all_unknown() {
        let facts = Facts::new();
        assert_eq!(facts.decided_count(), 0);
        for (id, cell) in facts.iter() {
            assert!(cell.val.is_unknown(), "{id:?} 默认必须是 unknown");
            assert!(cell.is_unset());
        }
    }

    #[test]
    fn unset_reads_unknown_not_false() {
        let facts = Facts::new();
        assert_eq!(facts.get(FactId::ImportsInjectionTrio), Tri::Unknown);
        assert_eq!(facts.num(FactId::SectionHighEntropy), None);
    }

    #[test]
    fn band_puts_ambiguous_measurements_in_unknown() {
        let band = Band::new(7.10, 7.60);
        let mut facts = Facts::new();
        facts.set_band(
            FactId::SectionHighEntropy,
            7.20,
            band,
            "max section entropy",
        );
        assert_eq!(facts.get(FactId::SectionHighEntropy), Tri::Unknown);
        facts.set_band(
            FactId::SectionHighEntropy,
            7.05,
            band,
            "max section entropy",
        );
        assert_eq!(facts.get(FactId::SectionHighEntropy), Tri::False);
        facts.set_band(
            FactId::SectionHighEntropy,
            7.80,
            band,
            "max section entropy",
        );
        assert_eq!(facts.get(FactId::SectionHighEntropy), Tri::True);
        assert_eq!(facts.num(FactId::SectionHighEntropy), Some(7.8));
    }

    #[test]
    fn table_is_copy_free_of_heap() {
        // 44 slots of (Tri, f32, &str) stays small enough to live on the stack
        // for every file in a scan.
        assert!(
            std::mem::size_of::<Facts>() <= 1200,
            "Facts grew to {} bytes",
            std::mem::size_of::<Facts>()
        );
    }
}
