//! The one way a draft attachment is turned into bytes.
//!
//! A draft is Markdown a person edits, and its `attachments:` list is therefore
//! caller-controlled text — the same trust level as the mail body. What it
//! names is read into a message that then leaves the machine, so the rules for
//! reading it are the difference between "this workspace sent a file from its
//! own case" and "this workspace sent whatever the draft pointed at".
//!
//! There used to be three copies of a purely lexical check — reject absolute
//! paths and anything that is not a `Normal` component, then join and
//! `fs::read` — one each in the planner, the SMTP builder and the store. None
//! of them looked at what was on disk, so `attachments: [files/link]` with
//! `files/link` a symbolic link to an SSH key was read, hashed, shown in the
//! plan as a harmless case-relative path, and attached. The plan's size and
//! hash were of the key.
//!
//! So: one primitive, and it opens rather than resolves. Callers get the open
//! handle, and the metadata comes from that handle rather than from the path,
//! so what was judged is what gets read.

use std::fs::{File, Metadata};
use std::path::{Path, PathBuf};

use crate::error::{AppError, Result};

/// The one directory a draft may attach from.
///
/// `attach` copies into `<case>/files/` and writes `files/<name>` into the
/// front matter, so this is not a new restriction on what the tool produces —
/// it is the restriction that was always implied and never enforced. Anything
/// else in the list is a hand-written path, and a hand-written path pointing
/// anywhere else is exactly the case that must not be read.
const MANAGED_DIRECTORY: &str = "files";

/// An attachment this workspace is willing to send.
#[derive(Debug)]
pub(crate) struct OpenAttachment {
    /// Where it is, for display in a plan.
    pub path: PathBuf,
    /// Its name, for the MIME part.
    pub filename: String,
    /// The open handle. Read from this, not from `path`.
    pub file: File,
    /// Taken from the handle.
    pub metadata: Metadata,
}

fn invalid(attachment: &str, reason: &str) -> AppError {
    AppError::new(
        "draft_invalid",
        format!("invalid draft attachment `{attachment}`: {reason}"),
    )
}

/// Open one attachment named by a draft, or refuse it.
///
/// Refuses anything that is not `files/<name>` naming a regular file, and
/// refuses to pass through a symbolic link at either component. On unix the
/// opened handle's identity is compared with what was checked, so a link
/// swapped in after the check is an error rather than a substitution.
pub(crate) fn open_draft_attachment(case_path: &Path, attachment: &str) -> Result<OpenAttachment> {
    let trimmed = attachment.trim();
    if trimmed.is_empty() {
        return Err(invalid(attachment, "it names nothing"));
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        return Err(invalid(attachment, "it is absolute"));
    }
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(part) => match part.to_str() {
                Some(part) => components.push(part),
                None => return Err(invalid(attachment, "it is not valid UTF-8")),
            },
            _ => return Err(invalid(attachment, "it is not a plain relative path")),
        }
    }
    let [directory, filename] = components.as_slice() else {
        return Err(invalid(
            attachment,
            "attachments live in `files/`; use `afmail draft attach` to put one there",
        ));
    };
    if *directory != MANAGED_DIRECTORY {
        return Err(invalid(
            attachment,
            "attachments live in `files/`; use `afmail draft attach` to put one there",
        ));
    }

    // Checked before it is opened, and each component separately: a link at
    // `files/` redirects every attachment in the case at once.
    let files_dir = case_path.join(MANAGED_DIRECTORY);
    match std::fs::symlink_metadata(&files_dir) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => return Err(invalid(attachment, "`files/` is not a directory")),
        Err(err) => return Err(AppError::io("inspect draft attachment directory", &err)),
    }
    let full = files_dir.join(filename);
    let checked = match std::fs::symlink_metadata(&full) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(invalid(attachment, "there is no such file in `files/`"));
        }
        Err(err) => return Err(AppError::io("inspect draft attachment", &err)),
    };
    if !checked.file_type().is_file() {
        return Err(invalid(
            attachment,
            "it is a symlink or another non-regular file, which this will not read",
        ));
    }

    let file = File::open(&full).map_err(|err| AppError::io("open draft attachment", &err))?;
    let metadata = file
        .metadata()
        .map_err(|err| AppError::io("inspect open draft attachment", &err))?;
    same_file(&checked, &metadata).ok_or_else(|| {
        invalid(
            attachment,
            "it changed between being checked and being opened",
        )
    })?;

    Ok(OpenAttachment {
        path: full,
        filename: (*filename).to_string(),
        file,
        metadata,
    })
}

/// Read an attachment's whole contents from the handle it was opened with.
pub(crate) fn read_attachment(open: &OpenAttachment) -> Result<Vec<u8>> {
    use std::io::Read as _;

    let expected_bytes = open.metadata.len();
    let mut bytes = Vec::new();
    let mut handle = (&open.file).take(expected_bytes.saturating_add(1));
    handle
        .read_to_end(&mut bytes)
        .map_err(|err| AppError::io("read draft attachment", &err))?;
    if bytes.len() as u64 != expected_bytes {
        return Err(AppError::new(
            "draft_invalid",
            "draft attachment changed size while being read; review the file and retry",
        ));
    }
    Ok(bytes)
}

/// Whether the file that was opened is the file that was checked.
///
/// This does not close the window between the two — that needs handle-relative
/// opens the standard library does not expose — but it turns a substitution
/// into an error instead of a silent swap.
#[cfg(unix)]
fn same_file(checked: &Metadata, opened: &Metadata) -> Option<()> {
    use std::os::unix::fs::MetadataExt as _;

    (checked.dev() == opened.dev() && checked.ino() == opened.ino()).then_some(())
}

#[cfg(not(unix))]
fn same_file(_checked: &Metadata, opened: &Metadata) -> Option<()> {
    // No portable file identity outside unix. The type check above still ran
    // against the path, and the handle is still what is read from.
    opened.is_file().then_some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(name: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!(
            "afmail-attach-{name}-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("files")).expect("case files directory");
        root
    }

    fn message(error: &AppError) -> String {
        format!("{error:?}")
    }

    #[test]
    fn attachments_that_grow_or_shrink_after_open_are_refused() {
        for contents in [b"larger than expected".as_slice(), b"x".as_slice()] {
            let root = case("changed-size");
            let path = root.join("files/report.txt");
            std::fs::write(&path, b"original").expect("original");
            let open = open_draft_attachment(&root, "files/report.txt").expect("open");
            std::fs::write(&path, contents).expect("external edit");
            let error = read_attachment(&open).expect_err("changed evidence");
            assert_eq!(error.error_code, "draft_invalid");
            std::fs::remove_dir_all(root).expect("cleanup");
        }
    }

    #[test]
    fn a_managed_file_opens_and_reads_from_the_handle() {
        let root = case("ok");
        std::fs::write(root.join("files").join("report.pdf"), b"report bytes").expect("a file");

        let open = open_draft_attachment(&root, "files/report.pdf").expect("an attachment");

        assert_eq!(open.filename, "report.pdf");
        assert_eq!(read_attachment(&open).expect("bytes"), b"report bytes");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_out_of_the_case_is_never_read() {
        // The shape a hand-edited draft can produce: the front matter names a
        // plain case-relative path, and the path is a link to a secret. The
        // plan would have shown `files/key` with the secret's size and hash.
        let root = case("escape");
        let outside = root.join("id_ed25519");
        std::fs::write(&outside, b"PRIVATE KEY").expect("a secret");
        std::os::unix::fs::symlink(&outside, root.join("files").join("key")).expect("link");

        let error = open_draft_attachment(&root, "files/key").expect_err("must be refused");
        assert!(message(&error).contains("symlink"), "{}", message(&error));

        // And a link standing in for the whole managed directory.
        let elsewhere = root.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).expect("a directory");
        std::fs::write(elsewhere.join("key"), b"PRIVATE KEY").expect("a secret");
        std::fs::remove_dir_all(root.join("files")).expect("clear");
        std::os::unix::fs::symlink(&elsewhere, root.join("files")).expect("link");

        assert!(open_draft_attachment(&root, "files/key").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_the_managed_directory_is_readable() {
        let root = case("confined");
        std::fs::write(root.join("notes.md"), b"case notes").expect("a file");
        std::fs::create_dir_all(root.join("files").join("nested")).expect("a directory");
        std::fs::write(root.join("files").join("nested").join("x"), b"x").expect("a file");

        for attachment in [
            "notes.md",
            "files/nested/x",
            "../outside",
            "/etc/passwd",
            "",
            "   ",
        ] {
            assert!(
                open_draft_attachment(&root, attachment).is_err(),
                "`{attachment}` must be refused"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_attachment_says_so_rather_than_failing_obscurely() {
        let root = case("missing");
        let error = open_draft_attachment(&root, "files/absent.txt").expect_err("no such file");
        assert!(
            message(&error).contains("no such file"),
            "{}",
            message(&error)
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
