use super::lock::{command_lock, workspace_progress_command};
use super::purge::{resolve_purge_action, ResolvedPurgeAction};
use super::push::push_confirmed;
use crate::cli::{
    ArchiveAction, ArchiveCaseAction, ArchiveCaseNotesAction, ArchiveListAction,
    ArchiveMessageCommand, ArchiveMessageNotesAction, CaseCommand, CaseDraftAction,
    CaseNotesAction, Command, ConfigAction, DoctorAction, LogAction, MessageAction,
    MessageAttachmentAction, PushAction, RemoteAction, RenderAction, TriageAction,
};
use crate::error::{AppError, Result};
use crate::progress::{ProgressCallback, WorkspaceProgressSink};
use crate::store::Workspace;
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
        command => {
            let cwd = std::env::current_dir().map_err(|e| AppError::io("current dir", &e))?;
            let lock = command_lock(&command, &cwd)?;
            let _lock = WorkspaceLock::acquire(&lock.root, lock.mode)?;
            let mut workspace_progress = workspace_progress_command(&command)
                .map(|name| WorkspaceProgressSink::start(&lock.root, name));
            if lock.mode == LockMode::Exclusive
                && !matches!(command, Command::Init | Command::Doctor { .. })
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
        Command::Init => Workspace::at(cwd).init(),
        Command::Pull { ids } => Workspace::discover(cwd)?.pull_with_progress(&ids, progress),
        Command::Config { action } => {
            let ws = Workspace::discover(cwd)?;
            match action {
                ConfigAction::Show => ws.config_show(),
                ConfigAction::Get { key } => ws.config_get(&key),
                ConfigAction::Set { key, values } => ws.config_set(&key, &values),
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
            if dry_run && confirm {
                return Err(AppError::new(
                    "invalid_request",
                    "--dry-run cannot be used with --confirm",
                ));
            }
            match action {
                Some(PushAction::List) => ws.push_list(),
                Some(PushAction::Drafts {
                    dry_run: child_dry_run,
                    confirm: child_confirm,
                }) => {
                    let confirmed = push_confirmed(dry_run, confirm, child_dry_run, child_confirm)?;
                    ws.push_with_progress(crate::push_queue::PushMode::Drafts, confirmed, progress)
                }
                Some(PushAction::DraftsSend {
                    dry_run: child_dry_run,
                    confirm: child_confirm,
                }) => {
                    let confirmed = push_confirmed(dry_run, confirm, child_dry_run, child_confirm)?;
                    ws.push_with_progress(
                        crate::push_queue::PushMode::DraftsSend,
                        confirmed,
                        progress,
                    )
                }
                Some(PushAction::Archive {
                    dry_run: child_dry_run,
                    confirm: child_confirm,
                }) => {
                    let confirmed = push_confirmed(dry_run, confirm, child_dry_run, child_confirm)?;
                    ws.push_with_progress(crate::push_queue::PushMode::Archive, confirmed, progress)
                }
                Some(PushAction::Spam {
                    dry_run: child_dry_run,
                    confirm: child_confirm,
                }) => {
                    let confirmed = push_confirmed(dry_run, confirm, child_dry_run, child_confirm)?;
                    ws.push_with_progress(crate::push_queue::PushMode::Spam, confirmed, progress)
                }
                Some(PushAction::Trash {
                    dry_run: child_dry_run,
                    confirm: child_confirm,
                }) => {
                    let confirmed = push_confirmed(dry_run, confirm, child_dry_run, child_confirm)?;
                    ws.push_with_progress(crate::push_queue::PushMode::Trash, confirmed, progress)
                }
                None => {
                    let confirmed = push_confirmed(dry_run, confirm, false, false)?;
                    ws.push_with_progress(crate::push_queue::PushMode::All, confirmed, progress)
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
                MessageAction::Archive {
                    message_id,
                    archive_ref,
                    summary,
                    reason,
                } => ws.archive_message(
                    &message_id,
                    &archive_ref,
                    Some(summary.as_str()),
                    reason.as_deref(),
                ),
                MessageAction::Spam { message_id, reason } => {
                    ws.spam_message(&message_id, reason.as_deref())
                }
                MessageAction::Unspam { message_id, reason } => {
                    ws.unspam_message(&message_id, reason.as_deref())
                }
                MessageAction::Trash { message_id, reason } => {
                    ws.trash_message(&message_id, reason.as_deref())
                }
                MessageAction::Untrash { message_id, reason } => {
                    ws.untrash_message(&message_id, reason.as_deref())
                }
                MessageAction::Unarchive { message_id, reason } => {
                    ws.unarchive_message(&message_id, reason.as_deref())
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
                    args.reason.as_deref(),
                ),
                CaseCommand::List => ws.case_list(),
                CaseCommand::Show { case_ref } => ws.active_case_show(&case_ref),
                CaseCommand::Add {
                    case_ref,
                    message_id,
                    reason,
                } => ws.add_message_to_case(&case_ref, &message_id, reason.as_deref()),
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
                    } => ws.create_draft(&case_ref, &to, &cc, subject.as_deref()),
                    CaseDraftAction::Validate {
                        case_ref,
                        draft_name,
                    } => ws.validate_draft(&case_ref, &draft_name),
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
                CaseCommand::Compose {
                    case_ref,
                    draft_name,
                } => ws.compose_draft(&case_ref, &draft_name),
                CaseCommand::Reply {
                    case_ref,
                    message_id,
                    all,
                } => ws.reply_to_message(&case_ref, &message_id, all),
                CaseCommand::Merge {
                    case_ref,
                    other_case_ref,
                    reason,
                } => ws.merge_case(&case_ref, &other_case_ref, reason.as_deref()),
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
                RenderAction::Refresh => ws.render_refresh(),
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
