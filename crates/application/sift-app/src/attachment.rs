//! FR-10 and NFR-53 — listing what a message carries, and writing it somewhere the user
//! chose under a name the sender did not.
//!
//! # Nothing here fetches until somebody asks
//!
//! A message's structure costs a few kilobytes. The forty-megabyte attachment described in it
//! costs nothing until [`App::write_attachment`] runs, which is what makes "lazy download" a
//! property of the request pattern rather than an intention.
//!
//! # The warning is decided from three sources, and their disagreement is itself a source
//!
//! FR-10 is specific about this, and about why: the platform decides what happens when a file
//! is opened from its **extension**, not from the media type the sender declared. So a message
//! can declare `text/plain` on a name ending `.command` and defeat any type-based check
//! completely. The union of the declared type, the derived extension, and the sniffed content
//! is warned on — and a *mismatch* among them warns on its own, because a sender who labels an
//! executable as a document has said something about their intent that neither source says
//! alone.

use sift_foundation::identity::LocalId;
use sift_foundation::normalize;
use sift_foundation::state::ContentValue;
use sift_provider::adapter::RemoteMessageId;
use std::path::{Path, PathBuf};

use crate::App;

/// One attachment, as the reader lists it.
#[derive(Debug, Clone)]
pub struct Attachment {
    /// The identifier the adapter takes to fetch this part. Opaque above the adapter.
    pub part: String,
    /// `type/subtype` as the sender declared it. Advisory, and named as such.
    pub media_type: String,
    /// The sender's name, normalized for native chrome under NFR-54 — isolated rather than
    /// stripped, because what is shown is prose about a file rather than a path to one.
    pub display_name: ContentValue,
    /// The name a save would derive under NFR-53. Shown beside the sender's where the two
    /// differ, because a name that changed silently is a name the user did not agree to.
    pub file_name: String,
    /// What the provider says it costs. Advisory: L-13 bounds what is actually transferred.
    pub declared_size: u64,
    /// Why opening this needs a warning first, or that it does not.
    pub warning: Warning,
}

/// FR-10's three sources, kept separate rather than collapsed into a boolean.
///
/// Separate because the reader has to say *which* one fired: "this is a program" and "this
/// says it is a document and looks like a program" are different sentences, and the second is
/// the one worth reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Warning {
    /// The declared media type is an executable one.
    pub declared: bool,
    /// The extension of the name about to be written is an executable one. **This is the one
    /// that decides what the platform does**, and the one a type check never sees.
    pub extension: bool,
    /// The declared type and the extension describe different kinds of thing.
    pub disagrees: bool,
}

impl Warning {
    /// Whether FR-10's explicit warning is required before opening.
    #[must_use]
    pub const fn required(&self) -> bool {
        self.declared || self.extension || self.disagrees
    }
}

/// Extensions the platform will execute, or hand to something that will.
///
/// A list rather than a rule, and deliberately over-broad: the cost of warning about a file
/// that turns out to be inert is a dialog, and the cost of not warning is the whole machine.
const EXECUTABLE_EXTENSIONS: &[&str] = &[
    "app",
    "bat",
    "cmd",
    "com",
    "command",
    "cpl",
    "dll",
    "dmg",
    "exe",
    "hta",
    "iso",
    "jar",
    "js",
    "jse",
    "lnk",
    "msi",
    "osascript",
    "pkg",
    "pif",
    "ps1",
    "py",
    "reg",
    "scpt",
    "scr",
    "sh",
    "vb",
    "vbe",
    "vbs",
    "workflow",
    "ws",
    "wsf",
    "zsh",
];

/// Media types that are executable regardless of what they are called.
const EXECUTABLE_TYPES: &[&str] = &[
    "application/x-msdownload",
    "application/x-executable",
    "application/x-mach-binary",
    "application/vnd.microsoft.portable-executable",
    "application/x-sh",
    "application/x-shellscript",
    "application/x-apple-diskimage",
    "application/java-archive",
];

/// Types whose content is inert data by construction. A file declaring one of these and
/// carrying an executable extension is the mismatch FR-10 asks to be treated as suspicious.
fn is_inert_type(media_type: &str) -> bool {
    media_type.starts_with("text/")
        || media_type.starts_with("image/")
        || media_type.starts_with("audio/")
        || media_type.starts_with("video/")
        || matches!(
            media_type,
            "application/pdf" | "application/json" | "application/rtf"
        )
}

fn extension_of(file_name: &str) -> Option<String> {
    Path::new(file_name)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
}

/// Classify from the two sources available before a single byte is fetched.
///
/// The third — the sniff — needs the content, and the content is exactly what FR-10 does not
/// download to build a list. So it joins at [`App::write_attachment`], where the bytes are in
/// hand anyway.
#[must_use]
pub fn classify(media_type: &str, file_name: &str) -> Warning {
    let media_type = media_type.trim().to_ascii_lowercase();
    let extension = extension_of(file_name);
    let declared = EXECUTABLE_TYPES.contains(&media_type.as_str());
    let extension_executable = extension
        .as_deref()
        .is_some_and(|e| EXECUTABLE_EXTENSIONS.contains(&e));

    Warning {
        declared,
        extension: extension_executable,
        disagrees: (is_inert_type(&media_type) && extension_executable)
            || (declared && !extension_executable),
    }
}

/// The fourth source, available only once the bytes are here: what the content actually is.
///
/// Magic numbers only, and only unambiguous ones. A heuristic that guesses would produce
/// disagreements that are noise rather than signal, and FR-10 makes a disagreement a warning.
#[must_use]
pub fn sniff_is_executable(bytes: &[u8]) -> bool {
    const MAGIC: &[&[u8]] = &[
        b"MZ",               // PE
        b"\x7fELF",          // ELF
        b"\xfe\xed\xfa\xce", // Mach-O, all four orders
        b"\xfe\xed\xfa\xcf",
        b"\xce\xfa\xed\xfe",
        b"\xcf\xfa\xed\xfe",
        b"\xca\xfe\xba\xbe", // Mach-O universal
        b"#!",               // an interpreter line
    ];
    MAGIC.iter().any(|m| bytes.starts_with(m))
}

/// What a save will do, decided and shown **before** it happens.
///
/// NFR-53 requires the exact final path be shown before the write, which means the path has to
/// be resolved before the write — including the disambiguation, because `report.pdf` and
/// `report (2).pdf` are different answers to "where did my file go".
#[derive(Debug, Clone)]
pub struct SavePlan {
    pub message: LocalId,
    pub part: String,
    /// Carried so the write does not re-fetch the structure to re-derive what it already knew.
    pub media_type: String,
    /// The derived name, which is `final_path`'s last component.
    pub file_name: String,
    /// Exactly what will be written. Not a directory and a name for the user to compose.
    pub final_path: PathBuf,
    /// True where the derived name differs from the sender's, which is worth saying out loud.
    pub renamed: bool,
    pub declared_size: u64,
}

impl App {
    /// FR-10's list. Structure only — nothing is fetched.
    ///
    /// # Errors
    /// No account holds the message, or it has not been synced far enough to have a remote
    /// identifier.
    pub fn attachments(&mut self, id: LocalId) -> Result<Vec<Attachment>, String> {
        let (owner, remote) = self.remote_of(id)?;
        let account = self.account(&owner)?;
        let adapter = account
            .adapter
            .as_ref()
            .ok_or("this account has no provider behind it")?;
        let parts = adapter.structure(&remote).map_err(|e| e.to_string())?;
        Ok(parts
            .iter()
            .filter(|p| !p.is_body())
            .map(|p| {
                let raw = p.filename.as_deref().unwrap_or_default();
                let file_name = normalize::for_file_name(raw);
                Attachment {
                    part: p.id.clone(),
                    media_type: p.media_type.clone(),
                    display_name: normalize::for_display(raw),
                    warning: classify(&p.media_type, &file_name),
                    file_name,
                    declared_size: p.size,
                }
            })
            .collect())
    }

    /// Resolve where an attachment would be written, without writing it.
    ///
    /// The directory is the user's — it comes from the platform's own chooser, and Sift never
    /// picks one. What Sift decides is the *name*, and it decides it under NFR-53.
    ///
    /// # Errors
    /// The message or part is unknown, the directory is not one, or no free name exists inside
    /// the disambiguation bound.
    pub fn plan_attachment_save(
        &mut self,
        id: LocalId,
        part: &str,
        directory: &Path,
    ) -> Result<SavePlan, String> {
        if !directory.is_dir() {
            return Err(format!("{} is not a directory", directory.display()));
        }
        let attachment = self
            .attachments(id)?
            .into_iter()
            .find(|a| a.part == part)
            .ok_or("this message carries no such attachment")?;

        let final_path = free_path(directory, &attachment.file_name)
            .ok_or("every name in that directory is taken")?;
        let renamed = attachment
            .display_name
            .as_str()
            .trim_matches(|c| c == '\u{2068}' || c == '\u{2069}')
            != attachment.file_name;

        Ok(SavePlan {
            message: id,
            part: attachment.part,
            media_type: attachment.media_type,
            file_name: attachment.file_name,
            final_path,
            renamed,
            declared_size: attachment.declared_size,
        })
    }

    /// Fetch the part and write it to the planned path.
    ///
    /// **Nothing is overwritten**, and that is held by `create_new` rather than by a check —
    /// a check before a write is a race, and the file that appears between the two is the one
    /// somebody cared about.
    ///
    /// Returns the bytes written and the fourth classification source, now that the content
    /// exists to look at.
    ///
    /// # Errors
    /// The fetch failed, the path was taken between planning and writing, or the write did.
    pub fn write_attachment(&mut self, plan: &SavePlan) -> Result<(u64, Warning), String> {
        let (owner, remote) = self.remote_of(plan.message)?;
        let account = self.account(&owner)?;
        let adapter = account
            .adapter
            .as_ref()
            .ok_or("this account has no provider behind it")?;
        let bytes = adapter
            .fetch_part(&remote, &plan.part)
            .map_err(|e| e.to_string())?;

        use std::io::Write as _;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&plan.final_path)
            .map_err(|e| format!("{}: {e}", plan.final_path.display()))?;
        file.write_all(&bytes)
            .map_err(|e| format!("{}: {e}", plan.final_path.display()))?;

        let mut warning = classify(&plan.media_type, &plan.file_name);
        if sniff_is_executable(&bytes) {
            // The content is executable. Whether that *disagrees* with what was declared is
            // the part worth reporting: a `.sh` that looks like a script is honest, and a
            // `.pdf` that starts with `MZ` is the case FR-10 wrote the rule for.
            warning.disagrees |= !warning.declared && !warning.extension;
            warning.declared = true;
        }
        Ok((bytes.len() as u64, warning))
    }

    /// The remote identifier a stored message was fetched under, and the account holding it.
    fn remote_of(&mut self, id: LocalId) -> Result<(String, RemoteMessageId), String> {
        let owner = self
            .owner_of_stored(id)
            .ok_or("no account holds this message")?;
        let account = self.account(&owner)?;
        let remote: String = account
            .store
            .store
            .query_row(
                "SELECT remote_id FROM message WHERE id = ?1",
                rusqlite::params![id.to_bytes().to_vec()],
                |r| r.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten()
            .ok_or("this message has no remote identifier yet — sync first")?;
        Ok((owner, RemoteMessageId(remote)))
    }
}

/// The first name in `directory` that is not taken, disambiguated the way a person expects.
///
/// `report.pdf`, then `report (2).pdf`, then `report (3).pdf` — the suffix goes before the
/// extension rather than after it, because a file named `report.pdf (2)` opens with the wrong
/// application, which is the same class of failure NFR-53 is about.
fn free_path(directory: &Path, file_name: &str) -> Option<PathBuf> {
    let direct = directory.join(file_name);
    if !direct.exists() {
        return Some(direct);
    }
    let path = Path::new(file_name);
    let stem = path
        .file_stem()
        .map_or(file_name, |s| s.to_str().unwrap_or(file_name));
    let extension = path.extension().and_then(|e| e.to_str());

    (2..=99).find_map(|n| {
        let candidate = match extension {
            Some(e) => format!("{stem} ({n}).{e}"),
            None => format!("{stem} ({n})"),
        };
        let path = directory.join(candidate);
        (!path.exists()).then_some(path)
    })
}
