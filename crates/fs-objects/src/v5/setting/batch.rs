//! `SettingRows::apply`: a batch of operations in ONE transaction (all or nothing; log rows in the same transaction).
//!
//! Pattern: Command list over a Unit of Work. No undo journal: a failing operation rolls the whole transaction back ("right,
//! not a detour"). Used by import (parts B/C). Written by Claude (Anthropic) as a probe; see CHANGELOG.

use fs_types::Utc;

use super::rows::{clear_in, put_in};
use super::rows_index::apply as index_in;
use super::rows_instance::{outcome, put_instance_in};
use super::rows_lock::{lock_in, unlock_in};
use super::{Actor, IndexChange, Outcome, PlaceRow, Prim, ScopeRow, SettingRows, Writer};
use crate::v5::codec::invalid;
use crate::v5::error::Result;

/// One operation of a batch.
#[derive(Clone, Debug)]
pub enum SettingOp<'a> {
    /// A person value (`v = None` removes).
    Put {
        /// The profile.
        profile: i64,
        /// The scope.
        scope: ScopeRow<'a>,
        /// The place.
        place: PlaceRow,
        /// The key.
        key: &'a str,
        /// The value.
        v: Option<Prim>,
        /// Who wrote it.
        w: Writer<'a>,
    },
    /// Remove every value of a profile in a scope.
    ClearProfile {
        /// The profile.
        profile: i64,
        /// The scope.
        scope: ScopeRow<'a>,
    },
    /// A change of the profile index (as in `change_index`).
    Index(IndexChange),
    /// An instance default (needs an actor).
    PutInstance {
        /// The key.
        key: &'a str,
        /// The value (`None` clears).
        v: Option<Prim>,
    },
    /// Set and lock an instance default (needs an actor).
    Lock {
        /// The key.
        key: &'a str,
        /// The value.
        v: Prim,
        /// Why.
        reason: &'a str,
    },
    /// Unlock an instance default (needs an actor).
    Unlock {
        /// The key.
        key: &'a str,
        /// Why.
        reason: &'a str,
    },
}

impl SettingRows<'_> {
    /// All operations in ONE transaction, in order; the first failure rolls everything back (nothing written, no log row, no
    /// `rev`). `Changed` with one `rev + 1` if any operation changed something (person values and index changes count as
    /// changes), else `Same`. Instance operations need `actor`.
    ///
    /// # Errors
    /// The error of the failing operation; `Invalid` `settings_instance_log.by` `actor-missing` for an instance operation without
    /// an actor.
    pub async fn apply(
        &self,
        ops: &[SettingOp<'_>],
        actor: Option<Actor<'_>>,
        at: Utc,
    ) -> Result<Outcome> {
        let c = self.store.begin().await?;
        let r = async {
            let mut changed = false;
            for op in ops {
                let need =
                    || actor.ok_or_else(|| invalid("settings_instance_log.by", "actor-missing"));
                changed |= match op {
                    SettingOp::Put {
                        profile,
                        scope,
                        place,
                        key,
                        v,
                        w,
                    } => {
                        put_in(&c, *profile, *scope, *place, key, v.as_ref(), *w, at).await?;
                        true
                    }
                    SettingOp::ClearProfile { profile, scope } => {
                        clear_in(&c, *profile, *scope).await?;
                        true
                    }
                    SettingOp::Index(ch) => {
                        index_in(&c, ch, at).await?;
                        true
                    }
                    SettingOp::PutInstance { key, v } => {
                        put_instance_in(&c, key, v.as_ref(), need()?, at).await?
                    }
                    SettingOp::Lock { key, v, reason } => {
                        lock_in(&c, key, v, reason, need()?, at).await?
                    }
                    SettingOp::Unlock { key, reason } => {
                        unlock_in(&c, key, reason, need()?, at).await?
                    }
                };
            }
            outcome(&c, changed).await
        }
        .await;
        c.finish(r).await
    }
}
