use std::path::{Component, Path};

use rustix::fd::{AsFd, OwnedFd};
use rustix::fs::{Mode, OFlags, ResolveFlags, openat2};
use thiserror::Error;

use crate::protocol::{MAX_CONTENT_BYTES, MAX_RELATIVE_PATH_BYTES};

const RESOLVE_FLAGS: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);

#[derive(Debug, Error)]
pub(crate) enum InputError {
    #[error("tool input is invalid")]
    Invalid,
    #[error("relative path is invalid")]
    InvalidPath,
    #[error("content exceeds the supported limit")]
    ContentTooLarge,
}

pub(crate) struct WriteInput {
    pub relative_path: String,
    pub parent: String,
    pub basename: String,
    pub content: String,
}

pub(crate) fn parse_input(value: &serde_json::Value) -> Result<WriteInput, InputError> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct RawInput {
        relative_path: String,
        content: String,
    }

    let raw: RawInput = serde_json::from_value(value.clone()).map_err(|_| InputError::Invalid)?;
    if raw.content.len() > MAX_CONTENT_BYTES {
        return Err(InputError::ContentTooLarge);
    }
    let (parent, basename) = validate_relative_path(&raw.relative_path)?;
    Ok(WriteInput {
        relative_path: raw.relative_path,
        parent,
        basename,
        content: raw.content,
    })
}

fn validate_relative_path(value: &str) -> Result<(String, String), InputError> {
    if value.is_empty()
        || value.len() > MAX_RELATIVE_PATH_BYTES
        || value.starts_with('/')
        || value.ends_with('/')
        || value.contains("//")
        || value.chars().any(char::is_control)
        || value
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(InputError::InvalidPath);
    }

    let path = Path::new(value);
    let mut normal = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) if !part.is_empty() && part.as_encoded_bytes().len() <= 255 => {
                normal.push(part.to_string_lossy().into_owned());
            }
            _ => return Err(InputError::InvalidPath),
        }
    }
    let basename = normal.pop().ok_or(InputError::InvalidPath)?;
    let parent = if normal.is_empty() {
        ".".to_owned()
    } else {
        normal.join("/")
    };
    Ok((parent, basename))
}

pub(crate) fn resolve_parent(
    workspace: impl AsFd,
    parent: &str,
) -> Result<OwnedFd, rustix::io::Errno> {
    openat2(
        workspace,
        parent,
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        resolve_flags(),
    )
}

pub(crate) fn resolve_flags() -> ResolveFlags {
    RESOLVE_FLAGS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_bounded_relative_target() {
        let parsed = parse_input(&serde_json::json!({
            "relative_path": "reports/result.txt",
            "content": "done"
        }))
        .expect("valid input must parse");
        assert_eq!(parsed.parent, "reports");
        assert_eq!(parsed.basename, "result.txt");
    }

    #[test]
    fn rejects_unsafe_or_ambiguous_paths() {
        for path in [
            "", "/tmp/x", "../x", "a/../x", "a//x", "a/", ".", "a/./x", "a\nx",
        ] {
            assert!(validate_relative_path(path).is_err(), "accepted {path:?}");
        }
    }

    #[test]
    fn rejects_content_over_four_kibibytes() {
        let value = serde_json::json!({
            "relative_path": "result.txt",
            "content": "x".repeat(MAX_CONTENT_BYTES + 1)
        });
        assert!(matches!(
            parse_input(&value),
            Err(InputError::ContentTooLarge)
        ));
    }

    #[test]
    fn path_resolution_policy_contains_every_mandatory_flag() {
        let flags = resolve_flags();
        assert!(flags.contains(ResolveFlags::BENEATH));
        assert!(flags.contains(ResolveFlags::NO_SYMLINKS));
        assert!(flags.contains(ResolveFlags::NO_MAGICLINKS));
        assert!(flags.contains(ResolveFlags::NO_XDEV));
    }
}
