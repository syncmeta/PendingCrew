//! Validate immutable original intent/result only. Current schema, targets,
//! edges, clock and archive state must never replace or invalidate this proof.
use super::{Result, StoreError, values};
use crate::{managed_links as graph, managed_tables_v2 as ids};
use crewd_proto::managed_tables_v3::*;
use std::collections::HashSet;
fn bad() -> StoreError {
    StoreError::Invalid("inconsistent managed table V3 original receipt".into())
}
fn id(v: &str, p: char) -> bool {
    ids::number(v, p).is_ok()
}
fn unique_ids(v: &[String], p: char, max: usize) -> bool {
    v.len() <= max
        && v.iter().all(|s| id(s, p))
        && v.iter().collect::<HashSet<_>>().len() == v.len()
}
fn edges(v: &[String]) -> bool {
    v.len() <= 4096
        && v.iter().all(|s| ids::revision(s).is_ok())
        && v.iter().collect::<HashSet<_>>().len() == v.len()
}
fn transition(before: &str, after: &str, changed: bool) -> bool {
    let Ok(n) = ids::revision(before) else {
        return false;
    };
    if changed {
        n.checked_add(1).is_some_and(|n| n.to_string() == after)
    } else {
        before == after
    }
}
fn option_scope(table: &str, field: &str, v: &OptionRef) -> bool {
    v.table_id == table && v.field_id == field && id(&v.option_id, 'e')
}
fn assigned_options(values: &[IssuedOption], field: &str, count: usize) -> bool {
    if values.len() != count {
        return false;
    }
    let mut prior = None;
    values.iter().all(|o| {
        let Ok(n) = ids::number(&o.option_id, 'e') else {
            return false;
        };
        if o.field_id != field || prior.is_some_and(|old: i64| old.checked_add(1) != Some(n)) {
            return false;
        }
        prior = Some(n);
        true
    })
}
fn requested_links(patches: &[Patch]) -> usize {
    patches
        .iter()
        .map(|p| match &p.change {
            CellPatch::ReplaceLinks { values, .. } => values.len(),
            _ => 0,
        })
        .sum()
}
fn patches_valid(patches: &[Patch]) -> bool {
    patches.len() <= 64
        && patches
            .iter()
            .map(|p| &p.field_id)
            .collect::<HashSet<_>>()
            .len()
            == patches.len()
        && patches.iter().all(|p| {
            id(&p.field_id, 'p')
                && match &p.change {
                    CellPatch::ReplaceLinks { values, .. } => values.len() <= 64,
                    CellPatch::SetScalar { value } => match value {
                        Cell::Integer(s) => s.parse::<i64>().is_ok_and(|n| n.to_string() == *s),
                        Cell::Real(n) => n.is_finite(),
                        Cell::Decimal(s) => values::decimal_valid(s),
                        Cell::Date(s) => values::date_valid(s),
                        Cell::Url(s) => values::url_valid(s),
                        Cell::SingleSelect(v) => {
                            id(&v.table_id, 'T')
                                && v.field_id == p.field_id
                                && id(&v.option_id, 'e')
                        }
                        _ => true,
                    },
                    _ => true,
                }
        })
}
fn patches_scope(patches: &[Patch], table: &str) -> bool {
    patches.iter().all(|p| match &p.change {
        CellPatch::SetScalar {
            value: Cell::SingleSelect(v),
        } => v.table_id == table,
        _ => true,
    })
}
fn schema_result(
    table: &str,
    change: &SchemaChange,
    fields: &[String],
    issued: &[IssuedOption],
    changed: bool,
) -> bool {
    if !unique_ids(fields, 'p', 64) {
        return false;
    }
    match change {
        SchemaChange::RenameOption {
            field_id, option, ..
        }
        | SchemaChange::DeactivateOption { field_id, option }
        | SchemaChange::RestoreOption { field_id, option }
        | SchemaChange::SetOptionTone {
            field_id, option, ..
        } => {
            if !option_scope(table, field_id, option) {
                return false;
            }
        }
        SchemaChange::AddOption { field_id, .. }
        | SchemaChange::RenameField { field_id, .. }
        | SchemaChange::HideField { field_id }
        | SchemaChange::RestoreField { field_id }
        | SchemaChange::SetTitleField { field_id }
        | SchemaChange::WidenSingleLink { field_id }
        | SchemaChange::WidenText { field_id }
        | SchemaChange::WidenNumberToText { field_id } => {
            if !id(field_id, 'p') {
                return false;
            }
        }
        _ => {}
    }
    if !changed {
        return fields.is_empty()
            && issued.is_empty()
            && !matches!(
                change,
                SchemaChange::AddField { .. }
                    | SchemaChange::AddOption { .. }
                    | SchemaChange::WidenNumberToText { .. }
            );
    }
    match change {
        SchemaChange::RenameTable { .. } | SchemaChange::SetPurpose { .. } => {
            fields.is_empty() && issued.is_empty()
        }
        SchemaChange::AddField { field } => {
            if fields.len() != 1 {
                return false;
            }
            let count = match &field.kind {
                KindDraft::SingleSelect { options } => options.len(),
                _ => 0,
            };
            assigned_options(issued, &fields[0], count)
        }
        SchemaChange::AddOption { field_id, .. } => {
            fields == [field_id.clone()] && assigned_options(issued, field_id, 1)
        }
        SchemaChange::RenameOption {
            field_id, option, ..
        }
        | SchemaChange::DeactivateOption { field_id, option }
        | SchemaChange::RestoreOption { field_id, option }
        | SchemaChange::SetOptionTone {
            field_id, option, ..
        } => {
            fields == [field_id.clone()]
                && issued.is_empty()
                && option_scope(table, field_id, option)
        }
        SchemaChange::RenameField { field_id, .. }
        | SchemaChange::HideField { field_id }
        | SchemaChange::RestoreField { field_id }
        | SchemaChange::SetTitleField { field_id }
        | SchemaChange::WidenSingleLink { field_id }
        | SchemaChange::WidenText { field_id }
        | SchemaChange::WidenNumberToText { field_id } => {
            fields == [field_id.clone()] && issued.is_empty()
        }
    }
}
pub fn event_witness(r: &Receipt) -> Result<(i64, Option<&str>, &str)> {
    let (source, kind, before, after) = match &r.effect {
        Effect::TableCreated {
            table_id,
            schema_revision,
            ..
        } => (table_id, 'T', None, schema_revision.as_str()),
        Effect::SchemaChanged {
            table_id,
            original_schema_revision,
            result_schema_revision,
            ..
        }
        | Effect::TableLifecycle {
            table_id,
            original_schema_revision,
            result_schema_revision,
            ..
        } => (
            table_id,
            'T',
            Some(original_schema_revision.as_str()),
            result_schema_revision.as_str(),
        ),
        Effect::RowAdded {
            row_id,
            row_revision,
            ..
        } => (row_id, 'O', None, row_revision.as_str()),
        Effect::RowPatched {
            row_id,
            original_row_revision,
            result_row_revision,
            ..
        }
        | Effect::RowLifecycle {
            row_id,
            original_row_revision,
            result_row_revision,
            ..
        } => (
            row_id,
            'O',
            Some(original_row_revision.as_str()),
            result_row_revision.as_str(),
        ),
    };
    Ok((ids::number(source, kind)?, before, after))
}
pub fn validate_receipt(input: &Submit, r: &Receipt) -> Result<()> {
    if input.scope != r.scope
        || input.crew_id != r.crew_id
        || input.request_id != r.request_id
        || !crewd_proto::valid_remote_uuid(&r.request_id)
        || !crewd_proto::valid_remote_uuid(&r.scope.restore_epoch)
        || r.scope.db_id.is_empty()
        || r.scope.db_id.len() > 256
        || r.scope.db_id.chars().any(char::is_control)
        || r.crew_id.is_empty()
        || !chrono::DateTime::parse_from_rfc3339(&r.recorded_at)
            .is_ok_and(|t| crate::ts_from_ms(t.timestamp_millis()) == r.recorded_at)
        || graph::intent(
            SUBMIT,
            &r.scope,
            &r.crew_id,
            &r.request_id,
            &input.operation,
        )? != r.intent_digest
        || r.event_id
            .as_ref()
            .is_some_and(|id| !crewd_proto::valid_remote_uuid(id))
    {
        return Err(bad());
    }
    let (kind, changed, valid) = match (&input.operation, &r.effect) {
        (
            Operation::CreateTable {
                fields,
                title_field_index,
                ..
            },
            Effect::TableCreated {
                table_id,
                schema_revision,
                field_ids,
                title_field_id,
                issued_options,
            },
        ) => {
            if fields.is_empty() || fields.len() > 64 {
                return Err(bad());
            }
            let expected_fields = (1..=fields.len())
                .map(|i| format!("p{i}"))
                .collect::<Vec<_>>();
            let mut expected_options = Vec::new();
            let mut next = 1;
            for (i, f) in fields.iter().enumerate() {
                if let KindDraft::SingleSelect { options } = &f.kind {
                    for _ in options {
                        expected_options.push(IssuedOption {
                            field_id: format!("p{}", i + 1),
                            option_id: format!("e{next}"),
                        });
                        next += 1;
                    }
                }
            }
            (
                OperationKind::CreateTable,
                true,
                id(table_id, 'T')
                    && schema_revision == "1"
                    && !fields.is_empty()
                    && fields.len() <= 64
                    && *field_ids == expected_fields
                    && field_ids.get(*title_field_index as usize) == Some(title_field_id)
                    && *issued_options == expected_options,
            )
        }
        (
            Operation::AlterTable {
                table_id,
                expected_schema_revision,
                reason,
                change,
            },
            Effect::SchemaChanged {
                table_id: actual,
                original_schema_revision,
                result_schema_revision,
                changed_field_ids,
                issued_options,
                changed,
            },
        ) => (
            OperationKind::AlterTable,
            *changed,
            id(table_id, 'T')
                && actual == table_id
                && original_schema_revision == expected_schema_revision
                && ids::text(reason, REASON_SCALARS)
                && transition(original_schema_revision, result_schema_revision, *changed)
                && schema_result(
                    table_id,
                    change,
                    changed_field_ids,
                    issued_options,
                    *changed,
                ),
        ),
        (
            Operation::AddRow {
                table_id,
                expected_schema_revision,
                patches,
            },
            Effect::RowAdded {
                table_id: actual,
                row_id,
                schema_revision,
                row_revision,
                edge_ids,
            },
        ) => (
            OperationKind::AddRow,
            true,
            id(table_id, 'T')
                && actual == table_id
                && id(row_id, 'O')
                && ids::revision(expected_schema_revision).is_ok()
                && schema_revision == expected_schema_revision
                && row_revision == "1"
                && patches_valid(patches)
                && patches_scope(patches, table_id)
                && edges(edge_ids)
                && edge_ids.len() == requested_links(patches),
        ),
        (
            Operation::PatchRow {
                row_id,
                expected_schema_revision,
                expected_row_revision,
                reason,
                patches,
            },
            Effect::RowPatched {
                table_id,
                row_id: actual,
                schema_revision,
                original_row_revision,
                result_row_revision,
                changed_field_ids,
                added_edge_ids,
                removed_edge_ids,
                changed,
            },
        ) => {
            let changed_sources = patches
                .iter()
                .filter(|p| !matches!(p.change, CellPatch::Keep {}))
                .map(|p| p.field_id.as_str())
                .collect::<HashSet<_>>();
            (
                OperationKind::PatchRow,
                *changed,
                id(table_id, 'T')
                    && id(row_id, 'O')
                    && actual == row_id
                    && ids::revision(expected_schema_revision).is_ok()
                    && schema_revision == expected_schema_revision
                    && original_row_revision == expected_row_revision
                    && transition(original_row_revision, result_row_revision, *changed)
                    && ids::text(reason, REASON_SCALARS)
                    && patches_valid(patches)
                    && patches_scope(patches, table_id)
                    && unique_ids(changed_field_ids, 'p', 64)
                    && changed_field_ids
                        .iter()
                        .all(|id| changed_sources.contains(id.as_str()))
                    && edges(added_edge_ids)
                    && edges(removed_edge_ids)
                    && added_edge_ids
                        .iter()
                        .all(|id| !removed_edge_ids.contains(id))
                    && added_edge_ids.len() <= requested_links(patches)
                    && if *changed {
                        !changed_field_ids.is_empty()
                    } else {
                        changed_field_ids.is_empty()
                            && added_edge_ids.is_empty()
                            && removed_edge_ids.is_empty()
                    },
            )
        }
        (
            Operation::ArchiveTable {
                table_id,
                expected_schema_revision,
                reason,
            }
            | Operation::RestoreTable {
                table_id,
                expected_schema_revision,
                reason,
            },
            Effect::TableLifecycle {
                table_id: actual,
                original_schema_revision,
                result_schema_revision,
                archived,
                changed,
            },
        ) => {
            let archive = matches!(input.operation, Operation::ArchiveTable { .. });
            (
                if archive {
                    OperationKind::ArchiveTable
                } else {
                    OperationKind::RestoreTable
                },
                *changed,
                id(table_id, 'T')
                    && actual == table_id
                    && original_schema_revision == expected_schema_revision
                    && *archived == archive
                    && transition(original_schema_revision, result_schema_revision, *changed)
                    && ids::text(reason, REASON_SCALARS),
            )
        }
        (
            Operation::ArchiveRow {
                row_id,
                expected_schema_revision,
                expected_row_revision,
                reason,
            }
            | Operation::RestoreRow {
                row_id,
                expected_schema_revision,
                expected_row_revision,
                reason,
            },
            Effect::RowLifecycle {
                table_id,
                row_id: actual,
                schema_revision,
                original_row_revision,
                result_row_revision,
                archived,
                changed,
            },
        ) => {
            let archive = matches!(input.operation, Operation::ArchiveRow { .. });
            (
                if archive {
                    OperationKind::ArchiveRow
                } else {
                    OperationKind::RestoreRow
                },
                *changed,
                id(table_id, 'T')
                    && id(row_id, 'O')
                    && actual == row_id
                    && ids::revision(expected_schema_revision).is_ok()
                    && schema_revision == expected_schema_revision
                    && original_row_revision == expected_row_revision
                    && *archived == archive
                    && transition(original_row_revision, result_row_revision, *changed)
                    && ids::text(reason, REASON_SCALARS),
            )
        }
        _ => return Err(bad()),
    };
    if !valid || r.operation_kind != kind || r.event_id.is_some() != changed {
        return Err(bad());
    }
    Ok(())
}
