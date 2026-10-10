//! The machine guard in the session manager ([`crate::machine`]): its loop over Brigadier's
//! CLI processes, the rows it files in the threads, and new workers held while the machine is
//! strained.

use std::time::{Duration, Instant};

use super::SessionManager;
use super::watchdog::WorkerWatch;
use crate::machine::builds::{Note, Proc};
use crate::machine::{RECHECK, Row, TICK, proc_of};
use crate::model::{ConversationId, DomainEvent, MachineStep, MachineStepKind, MachineStepReason};
use crate::work::{Task, TaskId};
use crate::{Error, Result, now_ms};

/// "the Mac" on macOS; "the computer" elsewhere.
pub(crate) fn machine_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "the Mac"
    } else {
        "the computer"
    }
}

impl SessionManager {
    /// Lets go on what an earlier daemon left stopped, then looks at the machine and at
    /// Brigadier's process trees every [`TICK`] until the daemon quits.
    pub(super) async fn start_machine_watch(&self) {
        let watch = self.machine.clone();
        match tokio::task::spawn_blocking(move || watch.recover()).await {
            Ok(0) => {}
            Ok(count) => tracing::info!(count, "let go on builds an earlier run left stopped"),
            Err(err) => tracing::warn!(error = %err, "could not look for stopped builds"),
        }
        let manager = self.me.clone();
        self.spawn(async move {
            let mut tick = tokio::time::interval(TICK);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                let Some(manager) = manager.upgrade() else {
                    return;
                };
                if manager.admit().is_err() {
                    return;
                }
                let clis = manager.brigadier_clis();
                let watch = manager.machine.clone();
                let rows =
                    match tokio::task::spawn_blocking(move || watch.tick(&clis, Instant::now()))
                        .await
                    {
                        Ok(rows) => rows,
                        Err(err) => {
                            tracing::warn!(error = %err, "the machine watch failed a round");
                            continue;
                        }
                    };
                for row in rows {
                    manager.machine_row(row).await;
                }
            }
        });
    }

    /// The daemon quits: every build Brigadier stopped goes on.
    pub(super) async fn quit_machine_watch(&self) {
        let watch = self.machine.clone();
        if let Err(err) = tokio::task::spawn_blocking(move || watch.quit()).await {
            tracing::warn!(error = %err, "could not let stopped builds go on");
        }
    }

    /// Every CLI process Brigadier started, with its owner.
    fn brigadier_clis(&self) -> Vec<(String, Proc)> {
        let platform = self.runtime.platform();
        self.runtime
            .ledger()
            .processes()
            .into_iter()
            .filter(|(_, pid, _)| platform.processes().is_alive(*pid))
            .filter_map(|(owner, pid, started)| {
                let proc = match started {
                    Some(started) => Proc {
                        pid,
                        started_ms: started.round() as i64,
                    },
                    None => proc_of(&**platform, pid)?,
                };
                Some((owner, proc))
            })
            .collect()
    }

    /// Files a row in the thread of the command's CLI's conversation.
    async fn machine_row(&self, row: Row) {
        let (kind, reason, verb) = match row.note {
            Note::WaitingToCool(reason) => (
                MachineStepKind::WaitingToCool,
                reason,
                "waits for the machine",
            ),
            Note::WaitingForBuild => (
                MachineStepKind::WaitingForBuild,
                MachineStepReason::Heat,
                "waits for another build",
            ),
            Note::Paused => (
                MachineStepKind::Paused,
                MachineStepReason::Heat,
                "paused for the heat",
            ),
            Note::Resumed => (MachineStepKind::Resumed, MachineStepReason::Heat, "resumed"),
        };
        tracing::info!(owner = %row.owner, command = %row.command, "{verb}");
        let Some((kind_of, id)) = row.owner.split_once(':') else {
            return;
        };
        let (conversation_id, task_id, request_id) = match kind_of {
            "task" => {
                let task_id = TaskId(id.to_owned());
                let Some(live) = self.existing_task_live(&task_id) else {
                    return;
                };
                let request_id = self
                    .task_by_id(&live.conversation_id, &task_id)
                    .await
                    .ok()
                    .and_then(|task| task.request_id);
                (live.conversation_id.clone(), Some(task_id), request_id)
            }
            "orch" | "chat" | "session" => {
                let conversation_id = ConversationId(id.to_owned());
                let request_id = self.request_for(&conversation_id, None).await;
                (conversation_id, None, request_id)
            }
            // Brain jobs, research and the like have no thread.
            _ => return,
        };
        self.record_machine_step(
            &conversation_id,
            MachineStep {
                kind,
                reason,
                request_id,
                task_id,
                command: Some(row.command),
                at_ms: now_ms(),
                position: 0,
            },
        )
        .await;
    }

    async fn record_machine_step(&self, conversation_id: &ConversationId, step: MachineStep) {
        let event = DomainEvent::MachineStepped {
            conversation_id: conversation_id.clone(),
            step,
        };
        if let Err(err) = self
            .core
            .record_conversation(conversation_id, vec![event])
            .await
        {
            tracing::warn!(conversation = %conversation_id, error = %err, "could not store a machine row");
        }
    }

    /// Holds a new worker during serious heat or critical memory pressure, with a row in the
    /// thread, until it eases. Fails when the task ends meanwhile.
    pub(crate) async fn hold_while_strained(&self, task: &Task) -> Result<()> {
        if self.machine.eased_within(Duration::ZERO).await {
            return Ok(());
        }
        let mut previous_reason = None;
        loop {
            let reason = MachineStepReason::from_load(self.machine.guard.current());
            if previous_reason != Some(reason) {
                let words = match reason {
                    MachineStepReason::Heat => {
                        format!("Waiting for {} to cool down", machine_name())
                    }
                    MachineStepReason::Memory => "Waiting for memory to free up".to_owned(),
                };
                self.set_task_blocked(&task.id, Some(words)).await;
                self.record_machine_step(
                    &task.conversation_id,
                    MachineStep {
                        kind: MachineStepKind::WaitingToCool,
                        reason,
                        request_id: task.request_id.clone(),
                        task_id: Some(task.id.clone()),
                        command: None,
                        at_ms: now_ms(),
                        position: 0,
                    },
                )
                .await;
                previous_reason = Some(reason);
            }
            let eased = self.machine.eased_within(RECHECK).await;
            // Stopped meanwhile, or the daemon quits: it doesn't start, eased or not.
            let now = self.task_by_id(&task.conversation_id, &task.id).await?;
            if now.state.is_final() {
                return Err(Error::Invalid(format!("task-{} has ended", task.number)));
            }
            self.admit()?;
            if eased {
                break;
            }
        }
        self.set_task_blocked(&task.id, None).await;
        Ok(())
    }

    /// Whether serious heat or critical memory pressure holds new workers now.
    pub(crate) fn machine_strained(&self) -> bool {
        self.machine.guard.current().workers_held()
    }

    /// `watch` with the time its worker's commands were held for the machine counted as
    /// activity: a build Brigadier stopped is no stall of its worker.
    pub(crate) fn unheld(&self, task_id: &TaskId, watch: &WorkerWatch) -> WorkerWatch {
        let mut watch = watch.clone();
        if let Some(held) = self.machine.held_at_ms(&format!("task:{task_id}")) {
            watch.last_event_ms = watch.last_event_ms.max(held);
        }
        watch
    }
}
