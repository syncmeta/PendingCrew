//! The logical scalar codec shared by V3, legacy projections and derived search.
//! It never rewrites stored legacy values merely to project a wider type.
use crate::{Result, StoreError};
use crewd_proto::{managed_tables_v2 as v2, managed_tables_v3::*};
use std::collections::HashSet;

fn bad() -> StoreError {
    StoreError::Invalid("invalid managed table logical value or definition".into())
}
pub fn legacy_bounds() -> TextBounds {
    TextBounds {
        max_utf8_bytes: TEXT_UTF8_BYTES,
        max_unicode_scalars: None,
    }
}
pub fn text_fits(s: &str, b: &TextBounds) -> bool {
    s.len() <= b.max_utf8_bytes as usize
        && b.max_unicode_scalars
            .is_none_or(|n| s.chars().count() <= n as usize)
}
pub fn bounds_valid(b: &TextBounds) -> bool {
    b.max_utf8_bytes == TEXT_UTF8_BYTES
        && matches!(
            b.max_unicode_scalars,
            None | Some(TEXT_SCALARS) | Some(LONG_TEXT_SCALARS)
        )
}
/// Canonical exact decimal: validation is character arithmetic, never floating point.
pub fn decimal_valid(s: &str) -> bool {
    let unsigned = s.strip_prefix('-').unwrap_or(s);
    let mut pieces = unsigned.split('.');
    let integer = pieces.next().unwrap_or("");
    let fraction = pieces.next();
    if pieces.next().is_some()
        || integer.is_empty()
        || !integer.bytes().all(|b| b.is_ascii_digit())
        || (integer.len() > 1 && integer.starts_with('0'))
        || fraction.is_some_and(|f| {
            f.is_empty()
                || f.len() > DECIMAL_MAX_SCALE as usize
                || f.ends_with('0')
                || !f.bytes().all(|b| b.is_ascii_digit())
        })
    {
        return false;
    }
    if unsigned == "0" {
        return s == "0";
    }
    let coefficient = unsigned
        .bytes()
        .filter(|b| *b != b'.')
        .skip_while(|b| *b == b'0');
    let digits = coefficient.count();
    (1..=DECIMAL_PRECISION as usize).contains(&digits)
}
pub fn date_valid(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10
        || b[4] != b'-'
        || b[7] != b'-'
        || b.iter()
            .enumerate()
            .any(|(i, v)| i != 4 && i != 7 && !v.is_ascii_digit())
    {
        return false;
    }
    let y = s[..4].parse::<u32>().unwrap_or(0);
    let m = s[5..7].parse::<u32>().unwrap_or(0);
    let d = s[8..].parse::<u32>().unwrap_or(0);
    let max = match m {
        2 if y.is_multiple_of(400) || (y.is_multiple_of(4) && !y.is_multiple_of(100)) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => 0,
    };
    y > 0 && d > 0 && d <= max
}
pub fn url_valid(s: &str) -> bool {
    if s.is_empty()
        || s.len() > TEXT_UTF8_BYTES as usize
        || s.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return false;
    }
    url::Url::parse(s).is_ok_and(|u| {
        matches!(u.scheme(), "http" | "https")
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
    })
}
pub fn scalar_valid(table_id: &str, field: &Field, value: &Cell, fresh: bool) -> bool {
    match (&field.kind, value) {
        (_, Cell::Null) => field.nullable,
        (FieldKind::Text { bounds } | FieldKind::LongText { bounds }, Cell::Text(s)) => {
            text_fits(s, bounds)
        }
        (FieldKind::Integer {}, Cell::Integer(s)) => {
            s.parse::<i64>().is_ok_and(|n| n.to_string() == *s)
        }
        (FieldKind::Real {}, Cell::Real(v)) => v.is_finite(),
        (FieldKind::Decimal { .. }, Cell::Decimal(s)) => decimal_valid(s),
        (FieldKind::Date {}, Cell::Date(s)) => date_valid(s),
        (FieldKind::Boolean {}, Cell::Boolean(_)) => true,
        (FieldKind::Url {}, Cell::Url(s)) => url_valid(s),
        (FieldKind::SingleSelect { options }, Cell::SingleSelect(v)) => {
            v.table_id == table_id
                && v.field_id == field.id
                && options
                    .iter()
                    .any(|o| o.id == v.option_id && (!fresh || o.active))
        }
        _ => false,
    }
}
pub fn graph_field(f: &Field) -> bool {
    matches!(f.kind, FieldKind::Person { .. } | FieldKind::Link { .. })
}
pub fn from_v2_field(f: v2::Field) -> Field {
    Field {
        id: f.id,
        name: f.name,
        nullable: f.nullable,
        hidden: f.hidden,
        kind: match f.kind {
            v2::FieldKind::Text {} => FieldKind::Text {
                bounds: legacy_bounds(),
            },
            v2::FieldKind::Integer {} => FieldKind::Integer {},
            v2::FieldKind::Real {} => FieldKind::Real {},
            v2::FieldKind::Link { target, many } => FieldKind::Link { target, many },
        },
    }
}
/// Mixed schema32 authorities may contain untouched V2 definitions. Those exact
/// bytes mean legacy bounds; a fresh V3 definition always stores explicit bounds.
pub fn decode_fields(raw: &str) -> Result<Vec<Field>> {
    if raw.len() > DEFINITION_ENCODED_BYTES {
        return Err(bad());
    }
    let values: Vec<serde_json::Value> = serde_json::from_str(raw)?;
    values
        .into_iter()
        .map(|v| {
            if v.pointer("/kind/kind").and_then(|v| v.as_str()) == Some("text")
                && v.pointer("/kind/bounds").is_none()
            {
                Ok(from_v2_field(serde_json::from_value(v)?))
            } else {
                // The old integer/real/link definitions are byte-compatible.
                Ok(serde_json::from_value(v)?)
            }
        })
        .collect()
}
pub fn to_v2_field(f: &Field) -> Option<v2::Field> {
    Some(v2::Field {
        id: f.id.clone(),
        name: f.name.clone(),
        nullable: f.nullable,
        hidden: f.hidden,
        kind: match &f.kind {
            FieldKind::Text { .. } => v2::FieldKind::Text {},
            FieldKind::Integer {} => v2::FieldKind::Integer {},
            FieldKind::Real {} => v2::FieldKind::Real {},
            FieldKind::Link { target, many } => v2::FieldKind::Link {
                target: target.clone(),
                many: *many,
            },
            _ => return None,
        },
    })
}
pub fn from_v2_cell(c: &v2::Cell) -> Cell {
    match c {
        v2::Cell::Null => Cell::Null,
        v2::Cell::Text(s) => Cell::Text(s.clone()),
        v2::Cell::Integer(s) => Cell::Integer(s.clone()),
        v2::Cell::Real(v) => Cell::Real(*v),
    }
}
pub fn to_v2_cell(c: &Cell) -> Option<v2::Cell> {
    Some(match c {
        Cell::Null => v2::Cell::Null,
        Cell::Text(s) => v2::Cell::Text(s.clone()),
        Cell::Integer(s) => v2::Cell::Integer(s.clone()),
        Cell::Real(v) => v2::Cell::Real(*v),
        _ => return None,
    })
}
/// A widening changes the projected type, not old scalar storage. The caller
/// must supply the authoritative recorded original numeric kind, not guess it.
pub fn logical_cell(raw: Cell, numeric_codec: Option<&str>) -> Result<Cell> {
    Ok(match (numeric_codec, raw) {
        (Some("integer"), Cell::Integer(s))
            if s.parse::<i64>().is_ok_and(|n| n.to_string() == s) =>
        {
            Cell::Text(s)
        }
        (Some("real"), Cell::Real(n)) if n.is_finite() => Cell::Text(serde_json::to_string(&n)?),
        (Some("decimal"), Cell::Decimal(s)) if decimal_valid(&s) => Cell::Text(s),
        (Some(_), v @ (Cell::Null | Cell::Text(_))) => v,
        (None, v) => v,
        _ => return Err(bad()),
    })
}
pub fn fields_valid(fields: &[Field], title: &str, next_field: i64, next_option: i64) -> bool {
    if fields.is_empty()
        || fields.len() > MAX_FIELDS
        || next_field <= 0
        || next_option <= 0
        || !fields
            .iter()
            .any(|f| f.id == title && !f.hidden && !graph_field(f))
        || fields.iter().map(|f| &f.id).collect::<HashSet<_>>().len() != fields.len()
        || fields.iter().map(|f| &f.name).collect::<HashSet<_>>().len() != fields.len()
    {
        return false;
    }
    let mut options = HashSet::new();
    fields.iter().all(|f| {
        crate::managed_tables_v2::text(&f.name, FIELD_NAME_SCALARS)
            && crate::managed_tables_v2::number(&f.id, 'p').is_ok_and(|n| n < next_field)
            && match &f.kind {
                FieldKind::Text { bounds } | FieldKind::LongText { bounds } => bounds_valid(bounds),
                FieldKind::Decimal {
                    precision,
                    max_scale,
                    unit,
                    display_scale,
                } => {
                    *precision == DECIMAL_PRECISION
                        && *max_scale == DECIMAL_MAX_SCALE
                        && display_scale.is_none_or(|n| n <= 18)
                        && unit
                            .as_ref()
                            .is_none_or(|s| crate::managed_tables_v2::text(s, 32))
                }
                FieldKind::SingleSelect { options: os } => {
                    os.len() <= OPTION_HISTORY_PER_FIELD_MAX
                        && os.iter().filter(|o| o.active).count() <= OPTION_ACTIVE_MAX
                        && os
                            .iter()
                            .map(|o| o.label.to_ascii_lowercase())
                            .collect::<HashSet<_>>()
                            .len()
                            == os.len()
                        && os.iter().all(|o| {
                            crate::managed_tables_v2::text(&o.label, 120)
                                && crate::managed_tables_v2::number(&o.id, 'e')
                                    .is_ok_and(|n| n < next_option)
                                && options.insert(o.id.clone())
                        })
                }
                _ => true,
            }
    }) && serde_json::to_vec(fields).is_ok_and(|v| v.len() <= DEFINITION_ENCODED_BYTES)
}
/// Stable textual history/search projection. Selection labels are looked up in
/// the actual schema at the caller's declared cut, never stored into row cells.
pub fn scalar_text(value: &Cell) -> Result<(HistoryValueType, String)> {
    Ok(match value {
        Cell::Null => (HistoryValueType::Null, "null".into()),
        Cell::Text(s) => (HistoryValueType::Text, s.clone()),
        Cell::Integer(s) => (HistoryValueType::Integer, s.clone()),
        Cell::Real(n) if n.is_finite() => (HistoryValueType::Real, serde_json::to_string(n)?),
        Cell::Decimal(s) => (HistoryValueType::Decimal, s.clone()),
        Cell::SingleSelect(o) => (
            HistoryValueType::SingleSelect,
            serde_json::to_string(
                &serde_json::json!({"fieldId":o.field_id,"optionId":o.option_id,"tableId":o.table_id}),
            )?,
        ),
        Cell::Date(s) => (HistoryValueType::Date, s.clone()),
        Cell::Boolean(b) => (HistoryValueType::Boolean, b.to_string()),
        Cell::Url(s) => (HistoryValueType::Url, s.clone()),
        _ => return Err(bad()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_decimal_character_boundaries() {
        for s in [
            "0",
            "1",
            "-1",
            "0.000000000000000001",
            "12345678901234567890123456789012345678",
            "1.000000000000000001",
        ] {
            assert!(decimal_valid(s), "{s}");
        }
        for s in [
            "",
            "-0",
            "+1",
            "01",
            "1.",
            ".1",
            "1.0",
            "0.00",
            "1e2",
            "1E2",
            " 1",
            "1 ",
            "123456789012345678901234567890123456789",
            "0.0000000000000000001",
        ] {
            assert!(!decimal_valid(s), "{s}");
        }
    }
    #[test]
    fn dates_urls_and_legacy_text_are_not_normalized() {
        for s in ["0001-01-01", "2000-02-29", "2024-02-29", "9999-12-31"] {
            assert!(date_valid(s));
        }
        for s in [
            "0000-01-01",
            "1900-02-29",
            "2023-02-29",
            "2024-2-01",
            "2024-01-01Z",
        ] {
            assert!(!date_valid(s));
        }
        assert!(url_valid("https://example.test/a?x=1#f"));
        for s in [
            "https://a@host.test/",
            "https://host.test/ ",
            "javascript:alert(1)",
        ] {
            assert!(!url_valid(s));
        }
        assert!(text_fits(&"x".repeat(65536), &legacy_bounds()));
        assert!(!text_fits(&"x".repeat(65537), &legacy_bounds()));
        assert_eq!(
            logical_cell(Cell::Real(-0.0), Some("real")).unwrap(),
            Cell::Text("-0.0".into())
        );
        assert_eq!(
            logical_cell(Cell::Integer(i64::MIN.to_string()), Some("integer")).unwrap(),
            Cell::Text(i64::MIN.to_string())
        );
    }
}
