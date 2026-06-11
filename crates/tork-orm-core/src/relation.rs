//! Relationship descriptors.
//!
//! `#[relations]` generates an accessor that returns a [`Relation`] describing how
//! two models connect. A relation drives [`QuerySet::join`](crate::QuerySet::join)
//! (for filtering and aggregation) and, in a later commit, preloading.
//!
//! A relation is expressed as a directed key pair: a join is always
//! `from_table.from_column = to_table.to_column`, and `to_table` is the table
//! brought into the query.

use std::marker::PhantomData;

use crate::query::ast::Join;

/// The kind of association between two models.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationKind {
    /// The parent has many children that carry its key (one-to-many).
    HasMany,
    /// This model carries a key referencing one parent (many-to-one).
    BelongsTo,
}

/// Describes how a parent model `P` relates to a related model `C`.
///
/// Built by the accessors `#[relations]` generates, such as `User::posts()`.
pub struct Relation<P, C> {
    kind: RelationKind,
    from_table: &'static str,
    from_column: &'static str,
    to_table: &'static str,
    to_column: &'static str,
    _marker: PhantomData<fn() -> (P, C)>,
}

impl<P, C> Relation<P, C> {
    /// Builds a `has_many` relation: `parent.parent_key = child.child_key`.
    pub fn has_many(
        parent_table: &'static str,
        parent_key: &'static str,
        child_table: &'static str,
        child_key: &'static str,
    ) -> Self {
        Self {
            kind: RelationKind::HasMany,
            from_table: parent_table,
            from_column: parent_key,
            to_table: child_table,
            to_column: child_key,
            _marker: PhantomData,
        }
    }

    /// Builds a `belongs_to` relation: `local.local_key = parent.parent_key`.
    pub fn belongs_to(
        local_table: &'static str,
        local_key: &'static str,
        parent_table: &'static str,
        parent_key: &'static str,
    ) -> Self {
        Self {
            kind: RelationKind::BelongsTo,
            from_table: local_table,
            from_column: local_key,
            to_table: parent_table,
            to_column: parent_key,
            _marker: PhantomData,
        }
    }

    /// Returns the kind of this relation.
    pub fn kind(&self) -> RelationKind {
        self.kind
    }

    /// Returns the table this relation brings into a query when joined.
    pub fn target_table(&self) -> &'static str {
        self.to_table
    }

    /// Builds the join node for this relation.
    pub fn join_node(&self) -> Join {
        Join {
            table: self.to_table,
            left_table: self.from_table,
            left_column: self.from_column,
            right_table: self.to_table,
            right_column: self.to_column,
        }
    }
}
