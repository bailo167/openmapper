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
pub(crate) fn migrate(mut value: Value) -> Result<(Value, Option<u64>), ProjectError> {
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
    if version > CURRENT_VERSION {
        return Err(ProjectError::FutureVersion { found: version });
    }
    let original = version;
    let mut v = version;
    while v < CURRENT_VERSION {
        let step = usize::try_from(v - 1)
            .ok()
            .and_then(|i| STEPS.get(i))
            .ok_or(ProjectError::NoMigration(v))?;
        value = step(value)?;
        v += 1;
        if let Some(obj) = value.as_object_mut() {
            obj.insert("version".into(), Value::from(v));
        }
    }
    Ok((value, (original != CURRENT_VERSION).then_some(original)))
}
