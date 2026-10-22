//! The machine guard in the session manager ([`crate::machine`]): its loop reading the
//! machine, and new workers held while the machine is strained.

use std::time::Duration;

use super::SessionManager;
use crate::machine::{RECHECK, TICK};
use crate::model::{ConversationId, DomainEvent, MachineStep, MachineStepKind, MachineStepReason};
use crate::work::Task;
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
    /// Reads the machine's load every [`TICK`] until the daemon quits.
    pub(super) async fn start_machine_watch(&self) {
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
                let watch = manager.machine.clone();
                if let Err(err) = tokio::task::spawn_blocking(move || watch.guard.read()).await {
                    tracing::warn!(error = %err, "the machine watch failed a round");
                }
            }
        });
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
    /// thread each time what holds it changes, until it eases. Fails when the task ends
    /// meanwhile.
    pub(crate) async fn hold_while_strained(&self, task: &Task) -> Result<()> {
        if self.machine.eased_within(Duration::ZERO).await {
            return Ok(());
        }
        let mut previous_reason = None;
        loop {
            let load = self.machine.guard.current();
            if !load.workers_held() {
                break;
            }
            let reason = MachineStepReason::from_load(load);
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
}
