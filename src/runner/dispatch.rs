use super::lock::{command_lock, workspace_progress_command};
use super::purge::{resolve_purge_action, ResolvedPurgeAction};
use super::push::push_confirmed;
use crate::cli::{
    ArchiveAction, ArchiveCaseAction, ArchiveCaseNotesAction, ArchiveListAction,
    ArchiveMessageCommand, ArchiveMessageNotesAction, CaseCommand, CaseDraftAction,
    CaseNotesAction, Command, ConfigAction, ContactCommand, ContactEmailAction, ContactNotesAction,
    ContactPhoneAction, DoctorAction, LogAction, MessageAction, MessageAttachmentAction,
    PushAction, RemoteAction, RenderAction, TriageAction,
};
use crate::error::{AppError, Result};
use crate::progress::{ProgressCallback, WorkspaceProgressSink};
use crate::store::{DraftChange, NewDraft, Workspace};
use crate::workspace_lock::{LockMode, WorkspaceLock};
use serde_json::{json, Value};
use std::path::Path;

pub fn execute_command(command: Command) -> Result<Value> {
    execute_command_with_progress(command, None)
}

pub(super) fn execute_command_with_progress(
    command: Command,
    progress: Option<&mut ProgressCallback<'_>>,
) -> Result<Value> {
    match command {
        Command::Skill { action } => crate::skill_admin::handle_action(action),
        Command::Status => {
            let cwd = std::env::current_dir().map_err(|e| AppError::io("current dir", &e))?;
            execute_status(&cwd)
        }
        Command::Init { .. } => {
            let cwd = std::env::current_dir().map_err(|e| AppError::io("current dir", &e))?;
            execute_command_unlocked(command, &cwd, progress)
        }
        command => {
            let cwd = std::env::current_dir().map_err(|e| AppError::io("current dir", &e))?;
            let lock = command_lock(&command, &cwd)?;
            let _lock = WorkspaceLock::acquire(&lock.root, lock.mode)?;
            let mut workspace_progress = workspace_progress_command(&command)
                .map(|name| WorkspaceProgressSink::start(&lock.root, name));
            if lock.mode == LockMode::Exclusive
                && !matches!(command, Command::Init { .. } | Command::Doctor { .. })
            {
                if let Err(err) = Workspace::at(&lock.root).ensure_no_incomplete_transactions() {
                    if let Some(sink) = &mut workspace_progress {
                        sink.finish_failure(&err);
                    }
                    return Err(err);
                }
            }
            let mut progress = progress;
            let should_emit_progress = progress.is_some() || workspace_progress.is_some();
            let result = {
                let mut emit_progress = |phase: &str, fields: Value| {
                    if let Some(callback) = progress.as_deref_mut() {
                        callback(phase, fields.clone());
                    }
                    if let Some(sink) = &mut workspace_progress {
                        sink.update(phase, fields);
                    }
                };
                let progress = if should_emit_progress {
                    Some(&mut emit_progress as &mut ProgressCallback<'_>)
                } else {
                    None
                };
                execute_command_unlocked(command, &cwd, progress)
            };
            if let Some(sink) = &mut workspace_progress {
                match &result {
                    Ok(value) => sink.finish_success(value),
                    Err(err) => sink.finish_failure(err),
                }
            }
            result
        }
    }
}

fn execute_command_unlocked(
    command: Command,
    cwd: &Path,
    progress: Option<&mut ProgressCallback<'_>>,
) -> Result<Value> {
    match command {
        Command::Init { path } => Workspace::init_command(cwd, path.as_deref()),
        Command::Pull { ids } => Workspace::discover(cwd)?.pull_with_progress(&ids, progress),
        Command::Config { action } => {
            let ws = Workspace::discover(cwd)?;
            match action {
                ConfigAction::Show => ws.config_show(),
                ConfigAction::Get { key } => ws.config_get(&key),
                ConfigAction::Set { key, values } => ws.config_set(&key, &values),
                ConfigAction::Add { key, slug, fields } => ws.config_add(&key, &slug, &fields),
                ConfigAction::Remove { key, slug } => ws.config_remove(&key, &slug),
            }
        }
        Command::Remote { action } => {
            let ws = Workspace::discover(cwd)?;
            match action {
                RemoteAction::Test => ws.remote_test(),
                RemoteAction::Folders => ws.remote_folders(),
            }
        }
        Command::Push {
            dry_run,
            confirm,
            action,
        } => {
            let ws = Workspace::discover(cwd)?;
            match action {
                Some(PushAction::List) => ws.push_list(),
                None => {
                    let confirmed = push_confirmed(dry_run, confirm)?;
                    ws.push_with_progress(confirmed, progress)
                }
            }
        }
        Command::Status => execute_status(cwd),
        Command::Doctor { action } => {
            let ws = Workspace::discover(cwd)?;
            match action {
                None => ws.doctor(),
                Some(DoctorAction::Repair { confirm }) => ws.doctor_repair(confirm),
            }
        }
        Command::Purge {
            action,
            older_than_days,
        } => {
            let ws = Workspace::discover(cwd)?;
            match resolve_purge_action(action, older_than_days)? {
                ResolvedPurgeAction::Spam { older_than_days } => ws.purge_spam(older_than_days),
                ResolvedPurgeAction::Trash { older_than_days } => ws.purge_trash(older_than_days),
                ResolvedPurgeAction::Deleted { older_than_days } => {
                    ws.purge_deleted(older_than_days)
                }
                ResolvedPurgeAction::Discards { older_than_days } => {
                    ws.purge_discards(older_than_days)
                }
            }
        }
        Command::Skill { action } => crate::skill_admin::handle_action(action),
        Command::Triage { action } => {
            let ws = Workspace::discover(cwd)?;
            match action {
                TriageAction::List => ws.triage_list(),
            }
        }
        Command::Message { action } => {
            let ws = Workspace::discover(cwd)?;
            match action {
                MessageAction::Show { message_id } => ws.message_show(&message_id),
                MessageAction::Spam { message_id, reason } => {
                    ws.spam_message(&message_id, reason.as_deref())
                }
                MessageAction::Trash { message_id, reason } => {
                    ws.trash_message(&message_id, reason.as_deref())
                }
                MessageAction::Restore { message_id, reason } => {
                    ws.restore_message(&message_id, reason.as_deref())
                }
                MessageAction::Attachment { action } => match action {
                    MessageAttachmentAction::Fetch {
                        message_id,
                        part_id,
                    } => ws.fetch_message_attachment(&message_id, part_id.as_deref()),
                },
            }
        }
        Command::Case { action } => {
            let ws = Workspace::discover(cwd)?;
            match action {
                CaseCommand::Create(args) => ws.create_case(
                    &args.name,
                    args.group.as_deref(),
                    args.message.as_deref(),
                    args.summary.as_deref(),
                    args.reason.as_deref(),
                ),
                CaseCommand::List => ws.case_list(),
                CaseCommand::Show { case_ref } => ws.active_case_show(&case_ref),
                CaseCommand::Add {
                    case_ref,
                    message_id,
                    summary,
                    reason,
                } => ws.add_message_to_case(
                    &case_ref,
                    &message_id,
                    summary.as_deref(),
                    reason.as_deref(),
                ),
                CaseCommand::Move { case_ref, group } => ws.move_case(&case_ref, &group),
                CaseCommand::Rename {
                    case_ref,
                    name,
                    reason,
                } => ws.rename_active_case(&case_ref, &name, reason.as_deref()),
                CaseCommand::Notes { action } => match action {
                    CaseNotesAction::Show { case_ref } => ws.active_case_notes_show(&case_ref),
                    CaseNotesAction::Append { case_ref, text } => {
                        ws.active_case_notes_append(&case_ref, &text)
                    }
                    CaseNotesAction::Replace { case_ref, text } => {
                        ws.active_case_notes_replace(&case_ref, &text)
                    }
                },
                CaseCommand::Archive { case_ref, reason } => {
                    ws.archive_case(&case_ref, reason.as_deref())
                }
                CaseCommand::Trash { case_ref, reason } => {
                    ws.trash_case(&case_ref, reason.as_deref())
                }
                CaseCommand::Reopen { case_ref, reason } => {
                    ws.reopen_case(&case_ref, reason.as_deref())
                }
                CaseCommand::Tag {
                    case_ref,
                    tag,
                    reason,
                } => ws.tag_case(&case_ref, &tag, reason.as_deref()),
                CaseCommand::Untag {
                    case_ref,
                    tag,
                    reason,
                } => ws.untag_case(&case_ref, &tag, reason.as_deref()),
                CaseCommand::Draft { action } => match action {
                    CaseDraftAction::New {
                        case_ref,
                        to,
                        cc,
                        subject,
                        identity,
                        body,
                        body_file,
                    } => ws.create_draft(
                        &case_ref,
                        NewDraft {
                            to: &to,
                            cc: &cc,
                            subject: &subject,
                            identity: identity.as_deref(),
                            body: body.as_deref(),
                            body_file: body_file.as_deref(),
                        },
                    ),
                    CaseDraftAction::Reply {
                        case_ref,
                        message_id,
                        identity,
                        body,
                        body_file,
                        all,
                    } => ws.reply_to_message(
                        &case_ref,
                        &message_id,
                        all,
                        identity.as_deref(),
                        body.as_deref(),
                        body_file.as_deref(),
                    ),
                    CaseDraftAction::Change {
                        case_ref,
                        draft_name,
                        subject,
                        to,
                        cc,
                        clear_cc,
                        identity,
                        body,
                        body_file,
                    } => ws.change_draft(
                        &case_ref,
                        &draft_name,
                        DraftChange {
                            subject: subject.as_deref(),
                            to: &to,
                            cc: &cc,
                            clear_cc,
                            identity: identity.as_deref(),
                            body: body.as_deref(),
                            body_file: body_file.as_deref(),
                        },
                    ),
                    CaseDraftAction::Show {
                        case_ref,
                        draft_name,
                    } => ws.show_draft(&case_ref, &draft_name),
                    CaseDraftAction::Validate {
                        case_ref,
                        draft_name,
                    } => ws.validate_draft(&case_ref, &draft_name),
                    CaseDraftAction::Save {
                        case_ref,
                        draft_name,
                    } => ws.save_draft(&case_ref, &draft_name),
                    CaseDraftAction::Send {
                        case_ref,
                        draft_name,
                    } => ws.send_draft(&case_ref, &draft_name),
                    CaseDraftAction::Attach {
                        case_ref,
                        draft_name,
                        path,
                    } => ws.attach_file_to_draft(&case_ref, &draft_name, &path),
                    CaseDraftAction::Remove {
                        case_ref,
                        draft_name,
                        reason,
                    } => ws.remove_draft(&case_ref, &draft_name, reason.as_deref()),
                },
                CaseCommand::Merge {
                    case_ref,
                    other_case_ref,
                    reason,
                } => ws.merge_case(&case_ref, &other_case_ref, reason.as_deref()),
            }
        }
        Command::Contact { action } => {
            use crate::store::contacts::{ContactExtractSource, NewContact};
            let ws = Workspace::discover(cwd)?;
            match action {
                ContactCommand::Create(args) => ws.contact_create(NewContact {
                    name: &args.name,
                    group: args.group.as_deref(),
                    emails: &args.emails,
                    phones: &args.phones,
                    org: args.org.as_deref(),
                    role: args.role.as_deref(),
                    tags: &args.tags,
                }),
                ContactCommand::List {
                    group,
                    tag,
                    org,
                    role,
                } => ws.contact_list(
                    group.as_deref(),
                    tag.as_deref(),
                    org.as_deref(),
                    role.as_deref(),
                ),
                ContactCommand::Show { contact_ref } => ws.contact_show(&contact_ref),
                ContactCommand::Move { contact_ref, group } => {
                    ws.contact_move(&contact_ref, &group)
                }
                ContactCommand::Rename { contact_ref, name } => {
                    ws.contact_rename(&contact_ref, &name)
                }
                ContactCommand::Email { action } => match action {
                    ContactEmailAction::Add { contact_ref, email } => {
                        ws.contact_email_add(&contact_ref, &email)
                    }
                    ContactEmailAction::Remove { contact_ref, email } => {
                        ws.contact_email_remove(&contact_ref, &email)
                    }
                },
                ContactCommand::Phone { action } => match action {
                    ContactPhoneAction::Add { contact_ref, phone } => {
                        ws.contact_phone_add(&contact_ref, &phone)
                    }
                    ContactPhoneAction::Remove { contact_ref, phone } => {
                        ws.contact_phone_remove(&contact_ref, &phone)
                    }
                },
                ContactCommand::Tag { contact_ref, tag } => ws.contact_tag(&contact_ref, &tag),
                ContactCommand::Untag { contact_ref, tag } => ws.contact_untag(&contact_ref, &tag),
                ContactCommand::Notes { action } => match action {
                    ContactNotesAction::Show { contact_ref } => ws.contact_notes_show(&contact_ref),
                    ContactNotesAction::Append { contact_ref, text } => {
                        ws.contact_notes_append(&contact_ref, &text)
                    }
                    ContactNotesAction::Replace { contact_ref, text } => {
                        ws.contact_notes_replace(&contact_ref, &text)
                    }
                },
                ContactCommand::Archive {
                    contact_ref,
                    reason,
                } => ws.contact_archive_contact(&contact_ref, reason.as_deref()),
                ContactCommand::Reopen {
                    contact_ref,
                    group,
                    reason,
                } => ws.contact_reopen_contact(&contact_ref, group.as_deref(), reason.as_deref()),
                ContactCommand::Extract {
                    from_triage,
                    from_case,
                    all,
                    group,
                } => {
                    let source = if from_triage {
                        ContactExtractSource::FromTriage
                    } else if let Some(case_ref) = from_case {
                        ContactExtractSource::FromCase(case_ref)
                    } else if all {
                        ContactExtractSource::All
                    } else {
                        ContactExtractSource::FromTriage
                    };
                    ws.contact_extract(source, group.as_deref())
                }
            }
        }
        Command::Archive { action } => {
            let ws = Workspace::discover(cwd)?;
            match action {
                ArchiveAction::List { target: None } => ws.archive_list(),
                ArchiveAction::List {
                    target: Some(ArchiveListAction::Cases),
                } => ws.archive_list_cases(),
                ArchiveAction::List {
                    target: Some(ArchiveListAction::Messages),
                } => ws.archive_list_messages(),
                ArchiveAction::Message { action } => match action {
                    ArchiveMessageCommand::Create(args) => ws.create_archive_message_category(
                        &args.name,
                        args.message.as_deref(),
                        args.summary.as_deref(),
                        args.reason.as_deref(),
                    ),
                    ArchiveMessageCommand::Add {
                        archive_ref,
                        message_id,
                        summary,
                        reason,
                    } => ws.archive_message(
                        &message_id,
                        &archive_ref,
                        Some(summary.as_str()),
                        reason.as_deref(),
                    ),
                    ArchiveMessageCommand::Show { archive_ref } => {
                        ws.archive_message_show(&archive_ref)
                    }
                    ArchiveMessageCommand::Restore {
                        archive_ref,
                        message_id,
                        reason,
                    } => ws.archive_message_restore(&archive_ref, &message_id, reason.as_deref()),
                    ArchiveMessageCommand::Move {
                        archive_ref,
                        message_id,
                        new_archive_ref,
                        reason,
                    } => ws.archive_message_move(
                        &archive_ref,
                        &message_id,
                        &new_archive_ref,
                        reason.as_deref(),
                    ),
                    ArchiveMessageCommand::Rename {
                        archive_ref,
                        name,
                        reason,
                    } => ws.archive_message_rename(&archive_ref, &name, reason.as_deref()),
                    ArchiveMessageCommand::SetSummary {
                        archive_ref,
                        message_id,
                        summary,
                        reason,
                    } => ws.archive_message_set_summary(
                        &archive_ref,
                        &message_id,
                        &summary,
                        reason.as_deref(),
                    ),
                    ArchiveMessageCommand::Notes { action } => match action {
                        ArchiveMessageNotesAction::Show { archive_ref } => {
                            ws.archive_message_notes_show(&archive_ref)
                        }
                        ArchiveMessageNotesAction::Append { archive_ref, text } => {
                            ws.archive_message_notes_append(&archive_ref, &text)
                        }
                        ArchiveMessageNotesAction::Replace { archive_ref, text } => {
                            ws.archive_message_notes_replace(&archive_ref, &text)
                        }
                    },
                },
                ArchiveAction::Case { action } => match action {
                    ArchiveCaseAction::Show { case_ref } => ws.archive_case_show(&case_ref),
                    ArchiveCaseAction::Restore {
                        case_ref,
                        group,
                        reason,
                    } => ws.archive_case_restore(&case_ref, &group, reason.as_deref()),
                    ArchiveCaseAction::Trash { case_ref, reason } => {
                        ws.archive_case_trash(&case_ref, reason.as_deref())
                    }
                    ArchiveCaseAction::Rename {
                        case_ref,
                        name,
                        reason,
                    } => ws.archive_case_rename(&case_ref, &name, reason.as_deref()),
                    ArchiveCaseAction::Notes { action } => match action {
                        ArchiveCaseNotesAction::Show { case_ref } => {
                            ws.archive_case_notes_show(&case_ref)
                        }
                        ArchiveCaseNotesAction::Append { case_ref, text } => {
                            ws.archive_case_notes_append(&case_ref, &text)
                        }
                        ArchiveCaseNotesAction::Replace { case_ref, text } => {
                            ws.archive_case_notes_replace(&case_ref, &text)
                        }
                    },
                },
            }
        }
        Command::Render { action } => {
            let ws = Workspace::discover(cwd)?;
            match action {
                RenderAction::Refresh => ws.render_refresh_with_progress(progress),
                RenderAction::Templates { force } => ws.render_templates(force),
            }
        }
        Command::Log { action } => {
            let ws = Workspace::discover(cwd)?;
            match action {
                LogAction::List { limit } => ws.log_list(limit),
                LogAction::Tail => ws.log_tail(),
                LogAction::Message { message_id } => ws.log_message(&message_id),
                LogAction::Case { case_uid } => ws.log_case(&case_uid),
                LogAction::Archive { archive_uid } => ws.log_archive(&archive_uid),
            }
        }
        #[cfg(feature = "ui")]
        Command::Ui(_) => {
            let ws = Workspace::discover(cwd)?;
            let message = crate::ui::workspace_snapshot(&ws)?;
            serde_json::to_value(message).map_err(|e| AppError::json("serialize AFUI snapshot", &e))
        }
    }
}

fn execute_status(cwd: &Path) -> Result<Value> {
    let ws = Workspace::discover(cwd)?;
    let progress = crate::progress::workspace_status_progress(ws.root())?;
    let Some(_lock) = WorkspaceLock::try_acquire(ws.root(), LockMode::Shared)? else {
        return Ok(json!({
            "code": "status",
            "workspace_locked": true,
            "progress": progress,
            "hint": "Workspace counts are omitted while another afmail command is using this workspace; retry after it finishes for full counts."
        }));
    };
    let mut status = ws.status()?;
    if let Value::Object(map) = &mut status {
        map.insert("progress".to_string(), progress);
    }
    Ok(status)
}
