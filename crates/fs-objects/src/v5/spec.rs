//! The table specification: one source for the `CREATE TABLE` text and for the registry (`fs_column`).
//!
//! Pattern: Value Object (const data); `ddl` is the only logic and derives text from data.

/// Logical column types of the v5 draft (section 2.1). The engine knows only `INTEGER`, `REAL`, `TEXT` and `BLOB`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Logical {
    /// A whole number.
    Int,
    /// A finite floating-point number.
    Real,
    /// Text.
    Text,
    /// 0 or 1.
    Bool,
    /// Milliseconds since 1970-01-01T00:00:00Z.
    Utc,
    /// Seconds since 1970-01-01T00:00:00, read as wall clock without a zone.
    Local,
    /// Days since 1970-01-01.
    Date,
    /// A zone name.
    Zone,
    /// Bytes as they are: only `object_original.bytes` (ADR 0086 item 4).
    Blob,
    /// UUID text of an object.
    Ref,
    /// Absolute IRI as written.
    Iri,
    /// One of a fixed list of words.
    Enum(&'static [&'static str]),
}

impl Logical {
    /// The engine storage class.
    #[must_use]
    pub fn sql(self) -> &'static str {
        match self {
            Self::Int | Self::Bool | Self::Utc | Self::Local | Self::Date => "INTEGER",
            Self::Real => "REAL",
            Self::Blob => "BLOB",
            _ => "TEXT",
        }
    }

    /// The name stored in `fs_column.logical`.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Int => "int",
            Self::Real => "real",
            Self::Text => "text",
            Self::Bool => "bool",
            Self::Utc => "utc",
            Self::Local => "local",
            Self::Date => "date",
            Self::Zone => "zone",
            Self::Blob => "blob",
            Self::Ref => "ref",
            Self::Iri => "iri",
            Self::Enum(_) => "enum",
        }
    }
}

/// One column.
#[derive(Clone, Copy, Debug)]
pub struct Col {
    /// The column name.
    pub name: &'static str,
    /// The logical type.
    pub logical: Logical,
    /// May be empty.
    pub null: bool,
    /// Computed by code on every write (R11), never accepted from a caller as free input.
    pub derived: bool,
}

impl Col {
    /// A required column.
    #[must_use]
    pub const fn req(name: &'static str, logical: Logical) -> Self {
        Self {
            name,
            logical,
            null: false,
            derived: false,
        }
    }

    /// An optional column.
    #[must_use]
    pub const fn opt(name: &'static str, logical: Logical) -> Self {
        Self {
            name,
            logical,
            null: true,
            derived: false,
        }
    }

    /// A column the code computes.
    #[must_use]
    pub const fn derived(name: &'static str, logical: Logical, null: bool) -> Self {
        Self {
            name,
            logical,
            null,
            derived: true,
        }
    }
}

/// One table: columns, table constraints (primary key, foreign keys, `CHECK`), indexes.
#[derive(Clone, Copy, Debug)]
pub struct Table {
    /// The table name (starts with the prefix of its domain).
    pub name: &'static str,
    /// The columns, in storage order.
    pub cols: &'static [Col],
    /// Primary key, foreign keys, `CHECK`s.
    pub constraints: &'static [&'static str],
    /// `CREATE INDEX` statements.
    pub indexes: &'static [&'static str],
}

impl Table {
    /// `CREATE TABLE IF NOT EXISTS ... STRICT` (every v5 table is `STRICT`, section 2.1).
    #[must_use]
    pub fn ddl(&self) -> String {
        let mut parts: Vec<String> = self
            .cols
            .iter()
            .map(|c| {
                let null = if c.null { "" } else { " NOT NULL" };
                let check = match c.logical {
                    Logical::Bool => format!(" CHECK ({} IN (0,1))", c.name),
                    Logical::Enum(words) => {
                        let list: Vec<String> = words.iter().map(|w| format!("'{w}'")).collect();
                        format!(" CHECK ({} IN ({}))", c.name, list.join(","))
                    }
                    _ => String::new(),
                };
                format!("{} {}{}{}", c.name, c.logical.sql(), null, check)
            })
            .collect();
        parts.extend(self.constraints.iter().map(|s| (*s).to_owned()));
        format!(
            "CREATE TABLE IF NOT EXISTS {} ({}) STRICT",
            self.name,
            parts.join(", ")
        )
    }

    /// The column with this name.
    #[must_use]
    pub fn col(&self, name: &str) -> Option<&Col> {
        self.cols.iter().find(|c| c.name == name)
    }
}
