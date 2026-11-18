//! The migration report: numbers, what was held back and why, the backup, a note. A document, not stored data.
//!
//! Pattern: Value Object with a text rendering.

use std::collections::BTreeMap;
use std::fmt::Write;

/// One object that was held back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    /// The v4 id (UUID text).
    pub id: String,
    /// The object type.
    pub kind: String,
    /// Why it was not carried over.
    pub reason: String,
}

/// What a check run found out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// The v4 file (stays the active one until the switch).
    pub source: String,
    /// The backup of the v4 file.
    pub backup: String,
    /// The new v5 file.
    pub target: String,
    /// The number of objects in the v4 file: it equals converted + held back (the run stops otherwise).
    pub v4_total: usize,
    /// Objects converted and round-tripped, per object type.
    pub converted: BTreeMap<String, usize>,
    /// Objects held back, with the reason.
    pub held: Vec<Held>,
    /// Things not carried on purpose, per name (counted, never silent).
    pub skipped: BTreeMap<String, usize>,
    /// Instants whose microseconds were rounded to milliseconds.
    pub rounded: usize,
    /// How long the run took.
    pub millis: u128,
}

impl Report {
    /// The number of converted objects.
    #[must_use]
    pub fn converted_total(&self) -> usize {
        self.converted.values().sum()
    }

    /// The report as text.
    #[must_use]
    pub fn render(&self) -> String {
        let mut s = String::from("FreeSynergy migration report (v4 to v5)\n\n");
        let _ = writeln!(s, "v4 file (still the active one): {}", self.source);
        let _ = writeln!(s, "backup of the v4 file:          {}", self.backup);
        let _ = writeln!(s, "new v5 file:                    {}", self.target);
        let _ = writeln!(s, "duration:                       {} ms\n", self.millis);
        let _ = writeln!(
            s,
            "v4 objects: {} = converted {} + held back {}",
            self.v4_total,
            self.converted_total(),
            self.held.len()
        );
        let _ = writeln!(s, "converted: {}", self.converted_total());
        for (k, n) in &self.converted {
            let _ = writeln!(s, "  {k}: {n}");
        }
        let _ = writeln!(s, "held back: {}", self.held.len());
        for h in &self.held {
            let _ = writeln!(s, "  {} ({}): {}", h.id, h.kind, h.reason);
        }
        let _ = writeln!(s, "skipped: {}", self.skipped.values().sum::<usize>());
        for (k, n) in &self.skipped {
            let _ = writeln!(s, "  {k}: {n}");
        }
        let _ = writeln!(
            s,
            "rounded instants (microseconds to milliseconds): {}\n",
            self.rounded
        );
        s.push_str(
            "Nothing was changed in the v4 file. Held objects stay in it. The program keeps using the v4\n\
             file until a person confirms the switch; the v4 file and its backup are never deleted here.\n",
        );
        s
    }
}
