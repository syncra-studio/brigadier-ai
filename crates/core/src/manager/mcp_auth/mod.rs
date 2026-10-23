//! Sign-ins to remote MCP servers (docs/research/phase7-probes.md (j)): which of each agent's
//! stores holds a usable sign-in, for Settings, and sign-ins made in Brigadier.
//!
//! - Settings' reads never ask the user ([`Prober`] `quiet`): a store that would ask reads as
//!   unknown. A refusal is remembered (no read asks again) until the user asks for it
//!   ([`McpAuth::ask_again`]).
//! - A sign-in in Brigadier runs in the background: it can be cancelled while it waits for the
//!   browser, its page opened again, and its sign-in signed out of. Its link stays here.
//!
//! No token goes into a log, an event, an IPC answer, a command line or a file but the owning
//! CLI's own store and Brigadier's own Keychain item.

mod keychain;
mod locks;
mod oauth;
mod stores;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use brigadier_registry::{
    CodexStore, Owner, Probe, Roots, Server, SignInStores, Transport, sha256_hex,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[cfg(test)]
pub(crate) use keychain::Memory;
#[cfg(not(test))]
pub(crate) use keychain::system as system_keychain;
pub(crate) use keychain::{Keychain, StoreError};
use stores::{Credential, Stores};
pub(crate) use stores::{Homes, Place};

/// How long a store read stays known (a usable sign-in, or none), so Settings' polling reads
/// a store at most this often.
const KNOWN_FOR: Duration = Duration::from_secs(30);

/// How a sign-in in Brigadier went. It never carries a token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum McpSignInStatus {
    /// Waiting for the user to finish in their browser.
    Pending,
    /// Signed in: the sign-in serves every agent's workers.
    SignedIn,
    /// The user cancelled it: nothing was saved.
    Cancelled,
    /// Why, in a short plain sentence.
    Failed { reason: String },
}

type Slot = Arc<tokio::sync::Mutex<Option<Credential>>>;

/// Opens a link in the user's browser.
pub(crate) type Open = Arc<dyn Fn(&str) + Send + Sync>;

/// A sign-in in Brigadier, while it runs and after.
struct SignInRun {
    status: McpSignInStatus,
    /// Where it is kept.
    place: Place,
    /// Its sign-in page once made, to open again: it never leaves the daemon.
    link: Option<String>,
    open: Open,
    /// Stops it while it waits.
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
    /// Its sign-in is being kept: too late to cancel.
    saving: bool,
}

/// A store the user didn't let Brigadier read. Claude Code keeps every sign-in in one item.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Refusal {
    Claude,
    Place(Place),
}

impl Refusal {
    fn of(place: &Place) -> Self {
        match place {
            Place::Claude { .. } => Self::Claude,
            _ => Self::Place(place.clone()),
        }
    }
}

pub(crate) struct McpAuth {
    keychain: Mutex<Arc<dyn Keychain>>,
    /// What a store held when last read, for [`KNOWN_FOR`].
    known: Mutex<HashMap<Place, (Instant, Probe)>>,
    /// The sign-ins in use, by place: one renewal at a time each.
    live: Mutex<HashMap<Place, Slot>>,
    /// Sign-ins no longer reused, by place, with the hash of the access token they held: a
    /// new sign-in there (another token) is reused again.
    dead: Mutex<HashMap<Place, String>>,
    /// Stores the user didn't let Brigadier read, until they ask again.
    refused: Mutex<HashSet<Refusal>>,
    /// When the user signed in to Brigadier's own sign-ins (ms since the epoch), as read.
    since: Mutex<HashMap<Place, u64>>,
    sign_ins: Mutex<HashMap<String, SignInRun>>,
    http: reqwest::Client,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

fn fingerprint(credential: &Credential) -> String {
    sha256_hex(credential.access_token.expose())
}

/// Runs a store's blocking work off the async threads.
async fn off<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, StoreError> + Send + 'static,
) -> Result<T, StoreError> {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|err| Err(StoreError::Failed(format!("background work failed: {err}"))))
}

impl McpAuth {
    pub(crate) fn new(keychain: Arc<dyn Keychain>) -> Self {
        Self {
            keychain: Mutex::new(keychain),
            known: Mutex::default(),
            live: Mutex::default(),
            dead: Mutex::default(),
            refused: Mutex::default(),
            since: Mutex::default(),
            sign_ins: Mutex::default(),
            http: oauth::client(),
        }
    }

    /// Whether this system can keep Brigadier's own sign-ins.
    pub(crate) fn keeps_sign_ins(&self) -> bool {
        lock(&self.keychain).supported()
    }

    fn stores(&self, homes: Homes) -> Stores {
        Stores {
            keychain: lock(&self.keychain).clone(),
            homes,
        }
    }

    /// The stores as `roots` places them, read quietly (never asking the user) or not.
    pub(crate) fn prober(&self, roots: &Roots, codex_store: CodexStore, quiet: bool) -> Prober<'_> {
        Prober {
            auth: self,
            stores: self.stores(Homes::new(roots)),
            codex_store,
            quiet,
        }
    }

    fn slot(&self, place: &Place) -> Slot {
        lock(&self.live).entry(place.clone()).or_default().clone()
    }

    fn is_dead(&self, place: &Place, credential: &Credential) -> bool {
        lock(&self.dead).get(place) == Some(&fingerprint(credential))
    }

    /// Drops what is known of `place`, so it is read again.
    fn forget(&self, place: &Place) {
        lock(&self.known).remove(place);
        lock(&self.live).remove(place);
    }

    fn know(&self, place: &Place, probe: Probe) {
        lock(&self.known).insert(place.clone(), (Instant::now(), probe));
    }

    /// Notes when the user signed in, for Brigadier's own sign-in `credential` at `place`.
    fn note_since(&self, place: &Place, credential: Option<&Credential>) {
        if !matches!(place, Place::Brigadier { .. }) {
            return;
        }
        match credential.and_then(|credential| credential.since) {
            Some(since) => lock(&self.since).insert(place.clone(), since),
            None => lock(&self.since).remove(place),
        };
    }

    /// What a failed read of `place` says: a refusal is remembered.
    fn read_failed(&self, place: &Place, err: &StoreError) -> Probe {
        self.forget(place);
        match err {
            StoreError::Busy => Probe::Unknown,
            StoreError::Denied => {
                tracing::info!(
                    owner = place.owner(),
                    "reading a stored MCP sign-in was refused; it isn't read again until the user asks"
                );
                lock(&self.refused).insert(Refusal::of(place));
                Probe::Refused
            }
            StoreError::Unsupported => {
                self.know(place, Probe::Unsupported);
                Probe::Unsupported
            }
            StoreError::Failed(reason) => {
                tracing::warn!(owner = place.owner(), %reason, "a stored MCP sign-in couldn't be read");
                Probe::Unknown
            }
        }
    }

    /// What `place` holds: refused until the user asks again, known within [`KNOWN_FOR`], else
    /// read now. A read that changed what the proxy holds replaces it, and one that finds no
    /// usable sign-in drops it. A quiet read that would ask leaves a sign-in in use as usable.
    fn probe(&self, stores: &Stores, place: &Place, quiet: bool) -> Probe {
        if lock(&self.refused).contains(&Refusal::of(place)) {
            return Probe::Refused;
        }
        if let Some((at, probe)) = lock(&self.known).get(place)
            && at.elapsed() < KNOWN_FOR
        {
            return *probe;
        }
        match stores.read(place, quiet) {
            Ok(Some(credential)) if credential.usable() && !self.is_dead(place, &credential) => {
                self.know(place, Probe::Usable);
                self.note_since(place, Some(&credential));
                // Kept for the proxy, which then needs no second read. A renewal under way
                // holds the slot and writes the store itself.
                if let Ok(mut held) = self.slot(place).try_lock()
                    && !held
                        .as_ref()
                        .is_some_and(|held| held.same_tokens(&credential))
                {
                    *held = Some(credential);
                }
                Probe::Usable
            }
            Ok(_) => {
                self.forget(place);
                self.know(place, Probe::Absent);
                self.note_since(place, None);
                Probe::Absent
            }
            Err(StoreError::Busy) if quiet && self.in_use(place) => Probe::Usable,
            Err(err) => self.read_failed(place, &err),
        }
    }

    /// The proxy holds a usable sign-in for `place`.
    fn in_use(&self, place: &Place) -> bool {
        let slot = lock(&self.live).get(place).cloned();
        slot.is_some_and(|slot| {
            slot.try_lock()
                .is_ok_and(|held| held.as_ref().is_some_and(Credential::usable))
        })
    }

    /// A new sign-in in Brigadier to the server kept at `place`, opened with `open`: its id,
    /// and what cancels it, for [`Self::sign_in`]. A sign-in already waiting for that server
    /// is kept: its page opens again, and its id comes back alone.
    pub(crate) fn begin_sign_in(
        &self,
        place: Place,
        open: Open,
    ) -> (String, Option<tokio::sync::oneshot::Receiver<()>>) {
        if let Some(id) = self.pending_sign_in(&place) {
            let _ = self.reopen_sign_in(&id);
            return (id, None);
        }
        let (cancel, cancelled) = tokio::sync::oneshot::channel();
        let id = uuid::Uuid::new_v4().to_string();
        lock(&self.sign_ins).insert(
            id.clone(),
            SignInRun {
                status: McpSignInStatus::Pending,
                place,
                link: None,
                open,
                cancel: Some(cancel),
                saving: false,
            },
        );
        (id, Some(cancelled))
    }

    /// How sign-in `id` went; `None` for one Brigadier doesn't know.
    pub(crate) fn sign_in_status(&self, id: &str) -> Option<McpSignInStatus> {
        lock(&self.sign_ins).get(id).map(|run| run.status.clone())
    }

    /// The sign-in waiting for the user for the server kept at `place`, if one is.
    pub(crate) fn pending_sign_in(&self, place: &Place) -> Option<String> {
        lock(&self.sign_ins)
            .iter()
            .find(|(_, run)| run.place == *place && run.status == McpSignInStatus::Pending)
            .map(|(id, _)| id.clone())
    }

    /// When the user signed in to Brigadier's own sign-in at `place` (ms since the epoch), as
    /// last read.
    pub(crate) fn signed_in_at(&self, place: &Place) -> Option<u64> {
        lock(&self.since).get(place).copied()
    }

    /// Stops sign-in `id` while it waits: its callback listener closes and nothing is kept.
    /// One already over stays as it ended.
    pub(crate) fn cancel_sign_in(&self, id: &str) -> Result<(), String> {
        let mut runs = lock(&self.sign_ins);
        let run = runs.get_mut(id).ok_or("There's no such sign-in.")?;
        if run.status != McpSignInStatus::Pending {
            return Ok(());
        }
        if run.saving {
            return Err("The sign-in is already being saved.".into());
        }
        run.status = McpSignInStatus::Cancelled;
        run.link = None;
        if let Some(cancel) = run.cancel.take() {
            let _ = cancel.send(());
        }
        Ok(())
    }

    /// Opens sign-in `id`'s page in the browser again, while it waits.
    pub(crate) fn reopen_sign_in(&self, id: &str) -> Result<(), String> {
        let (link, open) = {
            let runs = lock(&self.sign_ins);
            let run = runs.get(id).ok_or("There's no such sign-in.")?;
            if run.status != McpSignInStatus::Pending {
                return Err("The sign-in is over.".into());
            }
            let link = run
                .link
                .clone()
                .ok_or("The sign-in page isn't ready yet.")?;
            (link, run.open.clone())
        };
        open(&link);
        Ok(())
    }

    /// Runs sign-in `id` to `server` in the user's browser until it ends or `cancelled`, then
    /// keeps the sign-in in Brigadier's own Keychain item.
    pub(crate) async fn sign_in(
        &self,
        id: &str,
        server: &Server,
        homes: Homes,
        cancelled: tokio::sync::oneshot::Receiver<()>,
    ) {
        let outcome = tokio::select! {
            outcome = self.signed_in(id, server, homes) => outcome,
            // Cancelled: its status says so already.
            _ = cancelled => return,
        };
        let status = match outcome {
            Ok(()) => McpSignInStatus::SignedIn,
            Err(reason) => {
                tracing::info!(server = %server.name, %reason, "signing in to an MCP server didn't finish");
                McpSignInStatus::Failed { reason }
            }
        };
        let mut runs = lock(&self.sign_ins);
        if let Some(run) = runs.get_mut(id)
            && run.status == McpSignInStatus::Pending
        {
            run.status = status;
            run.link = None;
            run.cancel = None;
        }
    }

    async fn signed_in(&self, id: &str, server: &Server, homes: Homes) -> Result<(), String> {
        let (Transport::Http { url, headers }, Some(place)) = (
            &server.transport,
            Place::of(Owner::Brigadier, server, CodexStore::default()),
        ) else {
            return Err("Only a remote server signs in.".into());
        };
        let open = |link: &str| {
            let open = lock(&self.sign_ins).get_mut(id).map(|run| {
                run.link = Some(link.to_owned());
                run.open.clone()
            });
            if let Some(open) = open {
                open(link);
            }
        };
        let credential = oauth::sign_in(&self.http, url, headers, server.oauth.as_ref(), &open)
            .await
            .map_err(|err| err.to_string())?;
        // Cancelled meanwhile: nothing is kept. Else it is too late to cancel.
        match lock(&self.sign_ins).get_mut(id) {
            Some(run) if run.status == McpSignInStatus::Pending => run.saving = true,
            _ => return Err("Cancelled.".into()),
        }
        let since = credential.since;
        let stores = self.stores(homes);
        let saved = {
            let place = place.clone();
            off(move || match &place {
                Place::Brigadier { identity, url } => stores.save_own(identity, url, &credential),
                _ => Err(StoreError::Failed("not Brigadier's own".into())),
            })
            .await
        };
        if let Err(err) = saved {
            tracing::warn!(reason = %err, "a sign-in couldn't be kept in the Keychain");
            return Err("The sign-in couldn't be saved in the Keychain.".into());
        }
        lock(&self.dead).remove(&place);
        self.forget(&place);
        if let Some(since) = since {
            lock(&self.since).insert(place, since);
        }
        Ok(())
    }

    /// Deletes Brigadier's own sign-in at `place`, if it has one. Claude's and Codex's own
    /// stores are never touched.
    pub(crate) async fn sign_out(&self, place: &Place, homes: Homes) -> Result<(), String> {
        let Place::Brigadier { identity, .. } = place else {
            return Err("Only Brigadier's own sign-in signs out here.".into());
        };
        let (stores, identity) = (self.stores(homes), identity.clone());
        off(move || stores.remove_own(&identity))
            .await
            .map_err(|err| {
                tracing::warn!(reason = %err, "Brigadier's sign-in couldn't be deleted");
                "The sign-in couldn't be deleted from the Keychain.".to_owned()
            })?;
        lock(&self.dead).remove(place);
        lock(&self.since).remove(place);
        self.forget(place);
        Ok(())
    }

    /// Forgets that the user refused reading `places` and what was read of them: the next read
    /// asks the system again.
    pub(crate) fn ask_again(&self, places: &[Place]) {
        let mut refused = lock(&self.refused);
        for place in places {
            refused.remove(&Refusal::of(place));
        }
        drop(refused);
        for place in places {
            self.forget(place);
        }
    }
}

/// The stored sign-ins as one reader sees them: quiet ones never ask the user.
pub(crate) struct Prober<'a> {
    auth: &'a McpAuth,
    stores: Stores,
    codex_store: CodexStore,
    quiet: bool,
}

impl SignInStores for Prober<'_> {
    fn probe(&self, owner: Owner, server: &Server) -> Probe {
        Place::of(owner, server, self.codex_store).map_or(Probe::Unknown, |place| {
            self.auth.probe(&self.stores, &place, self.quiet)
        })
    }
}

impl Prober<'_> {
    /// Where `owner` keeps its sign-in to `server`.
    pub(crate) fn place(&self, owner: Owner, server: &Server) -> Option<Place> {
        Place::of(owner, server, self.codex_store)
    }

    /// When the user signed in to `server` in Brigadier (ms since the epoch), as last read.
    pub(crate) fn signed_in_at(&self, server: &Server) -> Option<u64> {
        self.place(Owner::Brigadier, server)
            .and_then(|place| self.auth.signed_in_at(&place))
    }

    /// The sign-in in Brigadier waiting for the user for `server`, if one is.
    pub(crate) fn pending_sign_in(&self, server: &Server) -> Option<String> {
        self.place(Owner::Brigadier, server)
            .and_then(|place| self.auth.pending_sign_in(&place))
    }
}
