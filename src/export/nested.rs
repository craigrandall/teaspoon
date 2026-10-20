//! Embedded messages as nested message directories (M4f-2a), and the staging of one message's own
//! files, which top-level and embedded messages share.
//!
//! An embedded message (an attachment whose method is "embedded message") is written like a
//! message of its own, at the directory the planner gave it under `<message>/attachments/`:
//! `message.md`, `metadata.json`, the kept body files, and its own `attachments/` folder, which
//! may hold further embedded messages. The planner stops nesting at its depth cap; a message at
//! the cap gets a `metadata.json` that says it was not expanded, and nothing else (the planner
//! reserved room for exactly that). Everything is staged one message at a time, and attachment
//! bytes are streamed, exactly as for a top-level message.
//!
//! This is a child of `export.rs` so that it can use that module's private helpers and report.

use std::collections::BTreeMap;

use anyhow::Result;
use serde::Serialize;

use super::{ExportReport, name_record, rel_with, stage_attachments};
use crate::archive::{
    ATTACHMENT_EXTRACTED, AttachmentRecord, MESSAGE_MD_NAME, METADATA_JSON_NAME, NameRecord,
    SCHEMA_VERSION, body_outcome, message_metadata, render_message_md, to_json,
};
use crate::model::MessageContent;
use crate::naming::NameFlag;
use crate::plan::{PlanEntry, SourceMessage};
use crate::source_msg::{EmbeddedOpen, MsgAttachments};
use crate::stage::Stager;

/// `kind` in the `metadata.json` of an embedded message.
const KIND_EMBEDDED_MESSAGE: &str = "embedded_message";
/// Why a message at the depth cap was not expanded (a status reason, and the reason on its
/// parent's attachment record).
const REASON_DEPTH_CAPPED: &str = "embedded_depth_capped";
/// The embedded message's storage could not be opened as a message.
const REASON_UNREADABLE: &str = "embedded_message_unreadable";

/// What the staging of attachments needs to find: the plan's attachment and embedded-message
/// entries by source ID, and the `.msg` file the message came from (recorded, relative, in every
/// `metadata.json`, including those of embedded messages).
pub(super) struct StageCtx<'a> {
    pub(super) planned: &'a BTreeMap<u64, &'a PlanEntry>,
    pub(super) source_file: &'a str,
}

/// Where one message's files go, and what kind of message it is.
pub(super) struct MessageJob<'a> {
    /// The message directory, as components below the archive root.
    pub(super) components: &'a [String],
    pub(super) directory: NameRecord,
    /// An embedded message (counted and recorded as such) rather than a top-level one.
    pub(super) embedded: bool,
}

/// One embedded-message attachment: its source, its plan entry, and its position among its
/// parent's attachments.
pub(super) struct EmbeddedAt<'a> {
    pub(super) message: &'a SourceMessage,
    pub(super) entry: &'a PlanEntry,
    pub(super) index: usize,
}

/// Stages one message: its attachments first (`metadata.json` records each one's size and hash,
/// which are known only once its data has been copied), then `message.md`, `metadata.json`, and
/// the kept body files. Top-level messages feed the body counters; embedded ones are counted
/// apart, so the existing counters keep their meaning.
pub(super) fn stage_message(
    stager: &mut Stager,
    ctx: &StageCtx<'_>,
    job: &MessageJob<'_>,
    message: &SourceMessage,
    content: &MessageContent,
    reader: &mut MsgAttachments<'_>,
    report: &mut ExportReport,
) -> Result<()> {
    let outcome = body_outcome(&content.bodies);
    let records = stage_attachments(stager, ctx, message, content, reader, report)?;
    let mut meta = message_metadata(
        ctx.source_file,
        content,
        &outcome,
        job.directory.clone(),
        records,
    );
    if job.embedded {
        meta.kind = KIND_EMBEDDED_MESSAGE;
    }
    stager.add(
        rel_with(job.components, MESSAGE_MD_NAME),
        render_message_md(content, &outcome).as_bytes(),
    )?;
    stager.add(
        rel_with(job.components, METADATA_JSON_NAME),
        to_json(&meta)?.as_bytes(),
    )?;
    if job.embedded {
        report.embedded_messages_extracted += 1;
        report.embedded_messages_loss_total += outcome
            .rendered
            .loss
            .iter()
            .map(|loss| loss.count)
            .sum::<u64>();
    } else {
        report.message_directories += 1;
        match content.bodies.plain_text.as_deref() {
            None => report.bodies_plain_absent += 1,
            Some(text) if text.trim().is_empty() => report.bodies_plain_empty += 1,
            Some(_) => report.bodies_plain_present += 1,
        }
        report.note_body(&outcome);
    }
    for original in &outcome.originals {
        stager.add(rel_with(job.components, original.name), &original.bytes)?;
    }
    Ok(())
}

/// Stages one embedded message and fills in its parent's attachment record. Past the planner's
/// depth cap only a `metadata.json` is written; a storage that cannot be opened as a message is
/// recorded and nothing is written for it. Only a failure to write to staging is an error.
pub(super) fn stage_embedded(
    stager: &mut Stager,
    ctx: &StageCtx<'_>,
    at: &EmbeddedAt<'_>,
    reader: &mut MsgAttachments<'_>,
    report: &mut ExportReport,
    record: &mut AttachmentRecord,
) -> Result<()> {
    if at.entry.flags.contains(&NameFlag::DepthCapped) {
        stage_capped(stager, ctx, at)?;
        record.reason = Some(REASON_DEPTH_CAPPED);
        return Ok(());
    }
    let Some(open) = reader.open_embedded(at.index) else {
        record.reason = Some(REASON_UNREADABLE);
        return Ok(());
    };
    let EmbeddedOpen {
        content,
        attachments: mut inner,
    } = open;
    let job = MessageJob {
        components: &at.entry.components,
        directory: name_record(at.entry),
        embedded: true,
    };
    stage_message(stager, ctx, &job, at.message, &content, &mut inner, report)?;
    record.status = ATTACHMENT_EXTRACTED;
    record.reason = None;
    record.file = at.entry.components.last().cloned();
    Ok(())
}

/// The `metadata.json` of an embedded message the planner would not nest any deeper. It records
/// what is known from the source tree (the subject, the directory name, how many attachments
/// were not looked at) and why nothing else is there.
#[derive(Serialize)]
struct CappedEmbedded<'a> {
    schema_version: &'static str,
    kind: &'static str,
    source_file: &'a str,
    subject: Option<&'a str>,
    directory: NameRecord,
    /// Its direct attachments (embedded messages among them), none of which was extracted.
    attachments_not_extracted: usize,
    status: &'static str,
    status_reasons: Vec<&'static str>,
}

fn stage_capped(stager: &mut Stager, ctx: &StageCtx<'_>, at: &EmbeddedAt<'_>) -> Result<()> {
    let meta = CappedEmbedded {
        schema_version: SCHEMA_VERSION,
        kind: KIND_EMBEDDED_MESSAGE,
        source_file: ctx.source_file,
        subject: at.message.subject.as_deref(),
        directory: name_record(at.entry),
        attachments_not_extracted: at.message.attachments.len(),
        status: "partial",
        status_reasons: vec![REASON_DEPTH_CAPPED],
    };
    stager.add(
        rel_with(&at.entry.components, METADATA_JSON_NAME),
        to_json(&meta)?.as_bytes(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::path::{Path, PathBuf};

    use crate::export::{ExportOptions, ExportStatus, Preflight, run_export};
    use crate::source_msg::{EmbeddedOpen, OpenMessage, open_message};
    use crate::stage::{COPY_CHUNK_BYTES, STAGING_DIR};

    const TICKS: u64 = 133_000_000_000_000_000;

    fn entry(ty: u16, id: u16, value: [u8; 8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&ty.to_le_bytes());
        v.extend_from_slice(&id.to_le_bytes());
        v.extend_from_slice(&6u32.to_le_bytes());
        v.extend_from_slice(&value);
        v
    }

    fn utf16(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
    }

    fn put(comp: &mut cfb::CompoundFile<std::fs::File>, path: &str, bytes: &[u8]) {
        let mut stream = comp.create_stream(path).expect("create stream");
        stream.write_all(bytes).expect("write stream");
        stream.flush().expect("flush stream");
    }

    fn patterned(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    /// One attachment of a synthetic message: a by-value file, an embedded message, or a bare
    /// attachment of another method.
    struct Att<'a> {
        method: i32,
        name: Option<&'a str>,
        data: Option<&'a [u8]>,
        embedded: Option<Msg<'a>>,
    }

    /// A synthetic message: subject, plain body, recipients (SMTP addresses, all To), attachments.
    struct Msg<'a> {
        subject: &'a str,
        body: Option<&'a str>,
        recipients: Vec<&'a str>,
        attachments: Vec<Att<'a>>,
    }

    fn msg<'a>(subject: &'a str, body: Option<&'a str>, attachments: Vec<Att<'a>>) -> Msg<'a> {
        Msg {
            subject,
            body,
            recipients: Vec::new(),
            attachments,
        }
    }

    fn file_att<'a>(name: &'a str, data: &'a [u8]) -> Att<'a> {
        Att {
            method: 1,
            name: Some(name),
            data: Some(data),
            embedded: None,
        }
    }

    fn embedded_att(inner: Msg<'_>) -> Att<'_> {
        Att {
            method: 5,
            name: None,
            data: None,
            embedded: Some(inner),
        }
    }

    /// The streams of one message under `base` (`""` for the file's message). The properties
    /// stream header is 32 bytes for the file's message and 24 for an embedded one.
    fn write_streams(
        comp: &mut cfb::CompoundFile<std::fs::File>,
        base: &str,
        m: &Msg<'_>,
        header_len: usize,
    ) {
        let mut props = vec![0u8; header_len];
        props.extend(entry(0x0040, 0x0E06, TICKS.to_le_bytes()));
        put(comp, &format!("{base}/__properties_version1.0"), &props);
        put(
            comp,
            &format!("{base}/__substg1.0_0037001F"),
            &utf16(m.subject),
        );
        if let Some(body) = m.body {
            put(comp, &format!("{base}/__substg1.0_1000001F"), &utf16(body));
        }
        for (i, address) in m.recipients.iter().enumerate() {
            let storage = format!("{base}/__recip_version1.0_#{i:08X}");
            comp.create_storage(&storage)
                .expect("create recipient storage");
            let mut props = vec![0u8; 8];
            props.extend(entry(0x0003, 0x0C15, [1, 0, 0, 0, 0, 0, 0, 0]));
            put(comp, &format!("{storage}/__properties_version1.0"), &props);
            put(
                comp,
                &format!("{storage}/__substg1.0_39FE001F"),
                &utf16(address),
            );
        }
        for (i, a) in m.attachments.iter().enumerate() {
            let storage = format!("{base}/__attach_version1.0_#{i:08X}");
            comp.create_storage(&storage)
                .expect("create attachment storage");
            let mut props = vec![0u8; 8];
            let mut method = [0u8; 8];
            method[..4].copy_from_slice(&a.method.to_le_bytes());
            props.extend(entry(0x0003, 0x3705, method));
            put(comp, &format!("{storage}/__properties_version1.0"), &props);
            if let Some(name) = a.name {
                put(
                    comp,
                    &format!("{storage}/__substg1.0_3707001F"),
                    &utf16(name),
                );
            }
            if let Some(data) = a.data {
                put(comp, &format!("{storage}/__substg1.0_37010102"), data);
            }
            if let Some(inner) = &a.embedded {
                let inner_base = format!("{storage}/__substg1.0_3701000D");
                comp.create_storage(&inner_base)
                    .expect("create embedded storage");
                write_streams(comp, &inner_base, inner, 24);
            }
        }
    }

    fn write_msg(path: &Path, m: &Msg<'_>) {
        let mut comp = cfb::create(path).expect("create CFB");
        write_streams(&mut comp, "", m, 32);
    }

    /// `names[0]` holding `names[1]` as an embedded message, and so on down the list.
    fn chain<'a>(names: &[&'a str]) -> Msg<'a> {
        let attachments = if names.len() > 1 {
            vec![embedded_att(chain(&names[1..]))]
        } else {
            Vec::new()
        };
        msg(names[0], Some("text"), attachments)
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tsp-nest-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn never_asked(_: &Preflight) -> bool {
        panic!("the user must not be asked in this case");
    }

    fn export(input: &Path, out: &Path) -> ExportStatus {
        let options = ExportOptions {
            overwrite: false,
            source_hash: true,
            interactive: false,
        };
        run_export(input, out, &options, &mut never_asked).expect("export runs")
    }

    fn tree_files(root: &Path) -> BTreeMap<String, Vec<u8>> {
        fn walk(dir: &Path, prefix: &str, into: &mut BTreeMap<String, Vec<u8>>) {
            for entry in std::fs::read_dir(dir)
                .expect("read dir")
                .filter_map(|e| e.ok())
            {
                let name = entry.file_name().to_string_lossy().into_owned();
                let rel = if prefix.is_empty() {
                    name
                } else {
                    format!("{prefix}/{name}")
                };
                if entry.path().is_dir() {
                    walk(&entry.path(), &rel, into);
                } else {
                    into.insert(rel, std::fs::read(entry.path()).expect("read file"));
                }
            }
        }
        let mut files = BTreeMap::new();
        walk(root, "", &mut files);
        files
    }

    fn json_of(files: &BTreeMap<String, Vec<u8>>, key: &str) -> serde_json::Value {
        serde_json::from_slice(&files[key]).unwrap_or_else(|_| panic!("{key} is JSON"))
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    // ---- one level ---------------------------------------------------------------------

    #[test]
    fn an_embedded_message_becomes_a_nested_message_directory() {
        let dir = temp_dir("one");
        let file = dir.join("m.msg");
        let big = patterned(2 * COPY_CHUNK_BYTES + 5);
        let mut inner = msg(
            "Inner",
            Some("inner text"),
            vec![file_att("inner.bin", &big)],
        );
        inner.recipients = vec!["inner@example.com"];
        write_msg(
            &file,
            &msg(
                "Outer",
                Some("outer text"),
                vec![file_att("outer.bin", b"outer-bytes"), embedded_att(inner)],
            ),
        );
        let out = dir.join("out");
        assert_eq!(export(&file, &out), ExportStatus::Completed);
        assert!(!out.join(STAGING_DIR).exists());

        let files = tree_files(&out);
        let names: Vec<&str> = files.keys().map(String::as_str).collect();
        assert_eq!(
            names,
            vec![
                "m/Outer/attachments/Inner/attachments/inner.bin",
                "m/Outer/attachments/Inner/body.txt",
                "m/Outer/attachments/Inner/message.md",
                "m/Outer/attachments/Inner/metadata.json",
                "m/Outer/attachments/outer.bin",
                "m/Outer/body.txt",
                "m/Outer/message.md",
                "m/Outer/metadata.json",
                "m/folder.json",
            ]
        );
        assert_eq!(
            files["m/Outer/attachments/Inner/attachments/inner.bin"],
            big
        );
        assert_eq!(
            files["m/Outer/attachments/Inner/body.txt"].as_slice(),
            b"inner text"
        );
        let inner_md = String::from_utf8(files["m/Outer/attachments/Inner/message.md"].clone())
            .expect("UTF-8");
        assert_eq!(
            inner_md,
            "# Inner\n\n- **To:** `inner@example.com`\n- **Date:** 2022-06-18T04:26:40Z\n\n```text\ninner text\n```\n"
        );

        // The embedded message's own record.
        let inner_meta = json_of(&files, "m/Outer/attachments/Inner/metadata.json");
        assert_eq!(inner_meta["kind"], "embedded_message");
        assert_eq!(inner_meta["source_file"], "m.msg");
        assert_eq!(inner_meta["subject"], "Inner");
        assert_eq!(inner_meta["directory"]["original"], "Inner");
        assert_eq!(inner_meta["time_filetime"], TICKS);
        assert_eq!(
            inner_meta["envelope"]["recipients"][0]["smtp_address"],
            "inner@example.com"
        );
        assert_eq!(inner_meta["attachments"][0]["status"], "extracted");
        assert_eq!(inner_meta["attachments"][0]["file"], "inner.bin");
        assert_eq!(inner_meta["attachments"][0]["bytes"], big.len());
        assert_eq!(inner_meta["attachments"][0]["sha256"], sha256_hex(&big));
        assert_eq!(inner_meta["attachments_not_extracted"], 0);

        // The outer message's record: its own recipients are not the embedded message's.
        let outer_meta = json_of(&files, "m/Outer/metadata.json");
        assert_eq!(outer_meta["kind"], "message");
        assert_eq!(outer_meta["envelope"]["recipients"], serde_json::json!([]));
        assert_eq!(outer_meta["attachments"][0]["file"], "outer.bin");
        let record = &outer_meta["attachments"][1];
        assert_eq!(record["method"], "embedded_message");
        assert_eq!(record["status"], "extracted");
        assert_eq!(record["file"], "Inner");
        assert!(record["reason"].is_null());
        assert!(record["bytes"].is_null() && record["sha256"].is_null());
        assert_eq!(outer_meta["attachments_not_extracted"], 0);
        assert_eq!(
            outer_meta["status_reasons"],
            serde_json::json!(["other_properties_not_preserved"])
        );

        let root = json_of(&files, "m/folder.json");
        assert_eq!(root["root"]["counts"]["messages"], 1);
        assert_eq!(root["root"]["counts"]["attachments_extracted"], 2);
        assert_eq!(root["root"]["counts"]["attachments_not_extracted"], 0);
        assert_eq!(root["root"]["counts"]["embedded_messages_not_extracted"], 0);
        assert_eq!(root["children"].as_array().expect("children").len(), 1);

        // A second run changes nothing and needs no consent.
        assert_eq!(export(&file, &out), ExportStatus::Completed);
        assert_eq!(tree_files(&out), files);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_embedded_messages_with_one_subject_get_their_own_directories() {
        let dir = temp_dir("dupes");
        let file = dir.join("d.msg");
        write_msg(
            &file,
            &msg(
                "Outer",
                None,
                vec![
                    embedded_att(msg("Same", Some("first"), vec![])),
                    embedded_att(msg("Same", Some("second"), vec![])),
                ],
            ),
        );
        let out = dir.join("out");
        assert_eq!(export(&file, &out), ExportStatus::Completed);
        let files = tree_files(&out);
        let meta = json_of(&files, "d/Outer/metadata.json");
        let first = meta["attachments"][0]["file"].as_str().expect("extracted");
        let second = meta["attachments"][1]["file"].as_str().expect("extracted");
        assert_ne!(first, second);
        assert_eq!(
            files[&format!("d/Outer/attachments/{first}/body.txt")].as_slice(),
            b"first"
        );
        assert_eq!(
            files[&format!("d/Outer/attachments/{second}/body.txt")].as_slice(),
            b"second"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- the depth cap -----------------------------------------------------------------

    #[test]
    fn nesting_stops_at_the_planners_depth_cap_and_the_cap_is_recorded() {
        let dir = temp_dir("cap");
        let file = dir.join("c.msg");
        // Outer holds L1, which holds L2, L3, L4, and L5. The planner nests three levels (L1 to
        // L3 are written in full); L4 is at the cap, and L5 is not planned at all.
        write_msg(&file, &chain(&["Outer", "L1", "L2", "L3", "L4", "L5"]));
        let out = dir.join("out");
        assert_eq!(export(&file, &out), ExportStatus::Completed);
        let files = tree_files(&out);

        let l3 = "c/Outer/attachments/L1/attachments/L2/attachments/L3";
        for level in [
            "c/Outer/attachments/L1",
            "c/Outer/attachments/L1/attachments/L2",
            l3,
        ] {
            assert!(
                files.contains_key(&format!("{level}/message.md")),
                "{level}"
            );
            assert_eq!(
                json_of(&files, &format!("{level}/metadata.json"))["kind"],
                "embedded_message"
            );
        }

        // L4: a metadata.json and nothing else, in the directory and below it.
        let l4 = format!("{l3}/attachments/L4");
        let under_l4: Vec<&str> = files
            .keys()
            .filter(|k| k.starts_with(&format!("{l4}/")))
            .map(String::as_str)
            .collect();
        assert_eq!(under_l4, vec![format!("{l4}/metadata.json").as_str()]);
        let capped = json_of(&files, &format!("{l4}/metadata.json"));
        assert_eq!(capped["kind"], "embedded_message");
        assert_eq!(capped["source_file"], "c.msg");
        assert_eq!(capped["subject"], "L4");
        assert_eq!(capped["directory"]["original"], "L4");
        assert!(
            capped["directory"]["adjustments"]
                .as_array()
                .expect("adjustments")
                .contains(&serde_json::json!("depth_capped"))
        );
        assert_eq!(capped["attachments_not_extracted"], 1);
        assert_eq!(capped["status"], "partial");
        assert_eq!(
            capped["status_reasons"],
            serde_json::json!(["embedded_depth_capped"])
        );

        // L3's record of L4 says why nothing is below it.
        let l3_meta = json_of(&files, &format!("{l3}/metadata.json"));
        assert_eq!(l3_meta["attachments"][0]["status"], "not_extracted");
        assert_eq!(l3_meta["attachments"][0]["reason"], "embedded_depth_capped");
        assert_eq!(l3_meta["attachments_not_extracted"], 1);
        assert!(
            l3_meta["status_reasons"]
                .as_array()
                .expect("status reasons")
                .contains(&serde_json::json!("attachments_not_extracted"))
        );

        let root = json_of(&files, "c/folder.json");
        assert_eq!(root["root"]["counts"]["embedded_messages_not_extracted"], 1);
        assert_eq!(root["root"]["counts"]["attachments_extracted"], 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- what is not a nested message --------------------------------------------------

    #[test]
    fn an_embedded_method_without_a_message_storage_stays_a_recorded_non_extraction() {
        let dir = temp_dir("broken");
        let file = dir.join("b.msg");
        write_msg(
            &file,
            &msg(
                "Outer",
                None,
                vec![Att {
                    method: 5,
                    name: Some("what.msg"),
                    data: None,
                    embedded: None,
                }],
            ),
        );
        let out = dir.join("out");
        assert_eq!(export(&file, &out), ExportStatus::Completed);
        let files = tree_files(&out);
        assert!(files.keys().all(|k| !k.contains("/attachments/")));
        let meta = json_of(&files, "b/Outer/metadata.json");
        assert_eq!(meta["attachments"][0]["status"], "not_extracted");
        assert_eq!(
            meta["attachments"][0]["reason"],
            "embedded_message_unreadable"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- the reader, directly ----------------------------------------------------------

    #[test]
    fn an_embedded_message_is_opened_in_its_own_scope() {
        let dir = temp_dir("reader");
        let file = dir.join("r.msg");
        let payload = patterned(70_000);
        let mut inner = msg(
            "Inner",
            Some("inner text"),
            vec![file_att("inner.bin", &payload)],
        );
        inner.recipients = vec!["inner@example.com"];
        write_msg(
            &file,
            &msg(
                "Outer",
                Some("outer text"),
                vec![file_att("outer.bin", b"outer-bytes"), embedded_att(inner)],
            ),
        );
        let OpenMessage {
            content,
            mut attachments,
        } = open_message(&file).expect("open");
        assert_eq!(content.subject.as_deref(), Some("Outer"));
        assert!(content.envelope.recipients.is_empty());

        assert!(
            attachments.open_embedded(0).is_none(),
            "a by-value attachment is not a message"
        );
        assert!(attachments.open_embedded(9).is_none());

        let EmbeddedOpen {
            content: inner,
            attachments: mut inner_reader,
        } = attachments.open_embedded(1).expect("embedded message");
        assert_eq!(inner.subject.as_deref(), Some("Inner"));
        assert_eq!(inner.bodies.plain_text.as_deref(), Some("inner text"));
        // The 24-byte header of an embedded properties stream is read, not the file's 32.
        assert_eq!(inner.time_filetime, Some(TICKS as i64));
        assert_eq!(inner.envelope.delivery_time, Some(TICKS as i64));
        assert_eq!(inner.envelope.recipients.len(), 1);
        assert_eq!(inner.attachments.len(), 1);
        assert_eq!(inner.attachments[0].data_len, Some(70_000));
        let (mut stream, len) = inner_reader.open_data(0).expect("data stream");
        assert_eq!(len, 70_000);
        let mut read_back = Vec::new();
        stream.read_to_end(&mut read_back).expect("read");
        assert_eq!(read_back, payload);
        drop(stream);
        assert!(inner_reader.open_embedded(0).is_none());
        drop(inner_reader);

        // The outer handle still reads its own attachment afterwards.
        let (mut stream, len) = attachments.open_data(0).expect("outer data");
        assert_eq!(len, 11);
        let mut outer_bytes = Vec::new();
        stream.read_to_end(&mut outer_bytes).expect("read");
        assert_eq!(outer_bytes, b"outer-bytes");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
