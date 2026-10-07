// SPDX-License-Identifier: Apache-2.0
//! Forward migrations between on-disk schema versions.
//!
//! Each migration is a pure `Value -> Value` step from version N to N+1.
//! Readers migrate old versions forward; writers only write [`CURRENT_VERSION`].

use serde_json::Value;

use crate::ProjectError;

pub const FORMAT: &str = "openmapper-project";
pub const CURRENT_VERSION: u64 = 1;

type Step = fn(Value) -> Result<Value, ProjectError>;

/// `STEPS[i]` migrates version `i + 1` to `i + 2`. Empty until version 2 exists.
const STEPS: &[Step] = &[];

/// Checks the format family and runs any needed migrations.
pub(crate) fn migrate(value: Value) -> Result<(Value, Option<u64>), ProjectError> {
    migrate_with(value, STEPS, CURRENT_VERSION)
}

/// [`migrate`] with an explicit step chain (`steps[i]` migrates version
/// `i + 1` to `i + 2`) and target version; lets tests exercise multi-step
/// (N-2 and older) migration before real versions exist.
pub(crate) fn migrate_with(
    mut value: Value,
    steps: &[Step],
    current: u64,
) -> Result<(Value, Option<u64>), ProjectError> {
    let format = value.get("format").and_then(Value::as_str);
    if format != Some(FORMAT) {
        return Err(ProjectError::WrongFormat {
            found: format.map(str::to_owned),
        });
    }
    let version = value
        .get("version")
        .and_then(Value::as_u64)
        .filter(|v| *v >= 1)
        .ok_or(ProjectError::BadVersion)?;
    if version > current {
        return Err(ProjectError::FutureVersion { found: version });
    }
    let original = version;
    let mut v = version;
    while v < current {
        let step = usize::try_from(v - 1)
            .ok()
            .and_then(|i| steps.get(i))
            .ok_or(ProjectError::NoMigration(v))?;
        value = step(value)?;
        v += 1;
        if let Some(obj) = value.as_object_mut() {
            obj.insert("version".into(), Value::from(v));
        }
    }
    Ok((value, (original != current).then_some(original)))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    // A synthetic history: v1 → v2 renames "title" to "name"; v2 → v3 adds
    // "tags" (default empty) and requires "name".
    fn v1_to_v2(mut v: Value) -> Result<Value, ProjectError> {
        if let Some(o) = v.as_object_mut()
            && let Some(t) = o.remove("title")
        {
            o.insert("name".into(), t);
        }
        Ok(v)
    }

    fn v2_to_v3(mut v: Value) -> Result<Value, ProjectError> {
        let o = v.as_object_mut().ok_or(ProjectError::BadVersion)?;
        if !o.contains_key("name") {
            return Err(ProjectError::NoMigration(2));
        }
        o.entry("tags").or_insert_with(|| json!([]));
        Ok(v)
    }

    const CHAIN: &[Step] = &[v1_to_v2, v2_to_v3];

    #[test]
    fn n_minus_two_documents_migrate_through_every_step() {
        let v1 = json!({"format": FORMAT, "version": 1, "title": "Show"});
        let (out, from) = migrate_with(v1, CHAIN, 3).unwrap();
        assert_eq!(from, Some(1));
        assert_eq!(
            out,
            json!({"format": FORMAT, "version": 3, "name": "Show", "tags": []})
        );

        let v2 = json!({"format": FORMAT, "version": 2, "name": "Show", "tags": ["a"]});
        let (out, from) = migrate_with(v2, CHAIN, 3).unwrap();
        assert_eq!(from, Some(2));
        assert_eq!(out["tags"], json!(["a"]));

        let v3 = json!({"format": FORMAT, "version": 3, "name": "Show"});
        assert_eq!(migrate_with(v3.clone(), CHAIN, 3).unwrap(), (v3, None));
    }

    #[test]
    fn gaps_future_versions_and_failing_steps_are_errors() {
        let v4 = json!({"format": FORMAT, "version": 4});
        assert!(matches!(
            migrate_with(v4, CHAIN, 3),
            Err(ProjectError::FutureVersion { found: 4 })
        ));
        let v1 = json!({"format": FORMAT, "version": 1, "title": "x"});
        assert!(matches!(
            migrate_with(v1, &CHAIN[..1], 3),
            Err(ProjectError::NoMigration(2))
        ));
        let broken = json!({"format": FORMAT, "version": 2});
        assert!(migrate_with(broken, CHAIN, 3).is_err());
        let zero = json!({"format": FORMAT, "version": 0});
        assert!(matches!(
            migrate_with(zero, CHAIN, 3),
            Err(ProjectError::BadVersion)
        ));
    }
}
