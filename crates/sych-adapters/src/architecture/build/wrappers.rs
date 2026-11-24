//! Which functions only wrap others, and which uses only read what they name.

use std::collections::HashMap;

use sych_core::architecture::{FileId, Item, ItemId, ItemKind, Reference, Wrapper};

use super::{Builder, Unit};
use crate::architecture::source::{Access, Plain, Pos, Shape};

/// Functions whose syntax only hands out or overwrites fields of `self`, or passes a call
/// on, whatever the index resolves those names to; a chain of forwarders never loops.
pub(super) fn wrappers(builder: &Builder<'_>, units: &[Unit<'_>]) -> HashMap<ItemId, Wrapper> {
    let unit_of: HashMap<FileId, &Unit<'_>> = units.iter().map(|u| (u.file, u)).collect();
    let mut wrappers = HashMap::new();
    for (&id, definition) in &builder.definitions {
        let item = &builder.items[id.index()];
        let (Some(shape), Some(file)) = (&definition.shape, item.file) else {
            continue;
        };
        let Some(unit) = unit_of.get(&file) else {
            continue;
        };
        let named = |pos: &Pos| {
            let index = *unit.at.get(pos)?;
            builder
                .symbols
                .get(unit.document.occurrences[index].symbol)
                .copied()
        };
        let field =
            |pos: &Pos| named(pos).filter(|f| builder.items[f.index()].kind == ItemKind::Field);
        let wrapper = match shape {
            Shape::Forward { callee, fields } => {
                let target = named(callee)
                    .filter(|t| *t != id && builder.items[t.index()].kind == ItemKind::Function);
                let fields: Option<Vec<ItemId>> = fields.iter().map(field).collect();
                target
                    .zip(fields)
                    .map(|(target, fields)| Wrapper::Forwarder { target, fields })
            }
            Shape::Access {
                lent,
                copied,
                to_read,
            } => {
                let lent: Option<Vec<ItemId>> = lent.iter().map(field).collect();
                let copied: Option<Vec<ItemId>> = copied.iter().map(field).collect();
                lent.zip(copied).map(|(lent, copied)| Wrapper::Accessor {
                    lent,
                    copied,
                    to_read: *to_read,
                })
            }
        };
        if let Some(wrapper) = wrapper.filter(|_| item.kind == ItemKind::Function) {
            wrappers.insert(id, wrapper);
        }
    }
    // Forwarders that end in a loop forward nothing.
    let looping: Vec<ItemId> = wrappers
        .keys()
        .copied()
        .filter(|&start| {
            let mut at = start;
            for _ in 0..=wrappers.len() {
                match wrappers.get(&at) {
                    Some(Wrapper::Forwarder { target, .. }) if *target == start => return true,
                    Some(Wrapper::Forwarder { target, .. }) => at = *target,
                    _ => return false,
                }
            }
            true
        })
        .collect();
    for id in looping {
        wrappers.remove(&id);
    }
    wrappers
}

/// Marks the references that only read: to a field, by what the code does with it and what
/// it holds; to an accessor that lends to read, directly or through forwarders, by what the
/// code does with the call.
pub(super) fn mark_copies(
    items: &[Item],
    references: &mut [Reference],
    accesses: &[Access],
    wrappers: &HashMap<ItemId, Wrapper>,
    plain: &HashMap<ItemId, Plain>,
) {
    let only_reads = |access: Access, fields: &[ItemId]| {
        let holds = |field: &ItemId| plain.get(field).copied().unwrap_or(Plain::No);
        match access {
            Access::Copy => true,
            Access::Borrow => fields.iter().all(|f| holds(f) != Plain::No),
            Access::Call => fields.iter().all(|f| holds(f) == Plain::Scalar),
            Access::Reach => false,
        }
    };
    for (reference, &access) in references.iter_mut().zip(accesses) {
        let item = reference.item;
        if items[item.index()].kind == ItemKind::Field {
            reference.copies = only_reads(access, &[item]);
            continue;
        }

        let mut called = item;
        for _ in 0..=wrappers.len() {
            match wrappers.get(&called) {
                Some(Wrapper::Forwarder { target, .. }) => called = *target,
                _ => break,
            }
        }
        if let Some(Wrapper::Accessor {
            lent,
            to_read: true,
            ..
        }) = wrappers.get(&called)
            && !lent.is_empty()
        {
            reference.copies = only_reads(access, lent);
        }
    }
}
