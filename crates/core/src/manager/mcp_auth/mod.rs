//! Sign-ins to remote MCP servers, shared between the agents (docs/research/phase7-probes.md
//! (j)). A server one agent is signed in to serves the other's workers, and a sign-in in
//! Brigadier serves every agent's:
//!
//! - Such a worker gets the server at Brigadier's proxy on 127.0.0.1 ([`proxy`]), one address
//!   per worker and server, which takes only that worker's own key. The proxy adds a fresh
//!   access token to each request: it renews the token when it is about to expire, or once
//!   after the server answers 401, and replays the request.
//! - A renewal takes the owning CLI's own lock, reads its store again (the CLI may have renewed
//!   it already), refreshes, and writes the new tokens back to the CLI's store, so the CLI
//!   keeps working when its authorization server rotates refresh tokens. A store that changed
//!   under it, a write that doesn't read back, or a refused refresh token ends the reuse.
//! - Settings' reads never ask the user ([`Prober`] `quiet`): a store that would ask reads as
//!   unknown. A worker that needs another agent's sign-in reads it as the system allows, which
//!   may show the Keychain's prompt once. A refusal is remembered (no read asks again) until
//!   the user asks for it ([`McpAuth::ask_again`]).
//! - A sign-in in Brigadier runs in the background: it can be cancelled while it waits for the
//!   browser, its page opened again, and its sign-in signed out of. Its link stays here.
//!
//! No token goes into a log, an event, an IPC answer, a command line or a file but the owning
//! CLI's own store and Brigadier's own Keychain item.

mod keychain;
mod locks;
mod oauth;
mod proxy;
mod stores;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use brigadier_registry::{
    CodexStore, OAuthClient, Owner, Probe, Roots, Secret, Server, SignInStores, Transport,
    sha256_hex,
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
/// An access token is renewed when it expires within this.
const RENEW_WITHIN: Duration = Duration::from_secs(60);

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

/// One worker's way to one server through the proxy.
#[derive(Debug, Clone)]
pub(crate) struct Route {
    /// The worker's key, the only one this route takes.
    pub grant: String,
    pub place: Place,
    pub url: String,
    /// The server's configured headers but `Authorization`, added to each request.
    pub headers: Vec<(String, Secret)>,
    pub client: Option<OAuthClient>,
    pub homes: Homes,
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

/// Whether a worker's key is still live.
pub(crate) type Alive = Arc<dyn Fn(&str) -> bool + Send + Sync>;

pub(crate) struct McpAuth {
    keychain: Mutex<Arc<dyn Keychain>>,
    alive: Alive,
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
    routes: Mutex<HashMap<String, Arc<Route>>>,
    proxy: tokio::sync::OnceCell<SocketAddr>,
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
    pub(crate) fn new(keychain: Arc<dyn Keychain>, alive: Alive) -> Self {
        Self {
            keychain: Mutex::new(keychain),
            alive,
            known: Mutex::default(),
            live: Mutex::default(),
            dead: Mutex::default(),
            refused: Mutex::default(),
            since: Mutex::default(),
            routes: Mutex::default(),
            proxy: tokio::sync::OnceCell::new(),
            sign_ins: Mutex::default(),
            http: oauth::client(),
        }
    }

    /// Tests: the Keychain the stores are read through.
    #[cfg(test)]
    pub(crate) fn set_keychain(&self, keychain: Arc<dyn Keychain>) {
        *lock(&self.keychain) = keychain;
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

    /// Stops reusing the sign-in `credential` at `place`.
    fn bury(&self, place: &Place, credential: &Credential) {
        lock(&self.dead).insert(place.clone(), fingerprint(credential));
        self.forget(place);
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

    /// Fails when the user refused reading `place`: nothing reads it until they ask again.
    fn not_refused(&self, place: &Place) -> Result<(), String> {
        if lock(&self.refused).contains(&Refusal::of(place)) {
            return Err(format!(
                "the system wasn't allowed to let Brigadier read {}'s sign-in",
                place.owner()
            ));
        }
        Ok(())
    }

    /// What `place` holds: refused until the user asks again, known within [`KNOWN_FOR`], a
    /// sign-in in use, else read now.
    fn probe(&self, stores: &Stores, place: &Place, quiet: bool) -> Probe {
        if lock(&self.refused).contains(&Refusal::of(place)) {
            return Probe::Refused;
        }
        if let Some((at, probe)) = lock(&self.known).get(place)
            && at.elapsed() < KNOWN_FOR
        {
            return *probe;
        }
        let slot = lock(&self.live).get(place).cloned();
        if let Some(slot) = slot
            && let Ok(held) = slot.try_lock()
            && held.as_ref().is_some_and(Credential::usable)
        {
            return Probe::Usable;
        }
        match stores.read(place, quiet) {
            Ok(Some(credential)) if credential.usable() && !self.is_dead(place, &credential) => {
                self.know(place, Probe::Usable);
                self.note_since(place, Some(&credential));
                // Kept for the proxy, which then needs no second read.
                if let Ok(mut held) = self.slot(place).try_lock()
                    && held.is_none()
                {
                    *held = Some(credential);
                }
                Probe::Usable
            }
            Ok(_) => {
                self.know(place, Probe::Absent);
                self.note_since(place, None);
                Probe::Absent
            }
            Err(err) => self.read_failed(place, &err),
        }
    }

    /// A route for `route`'s worker to its server through the proxy: its address.
    pub(crate) async fn route(self: &Arc<Self>, route: Route) -> Result<String, String> {
        let me = Arc::downgrade(self);
        let addr = self
            .proxy
            .get_or_try_init(|| proxy::start(me))
            .await
            .map_err(|err| format!("the sign-in proxy didn't start: {err}"))?;
        let id = uuid::Uuid::new_v4().simple().to_string();
        let mut routes = lock(&self.routes);
        routes.retain(|_, route| (self.alive)(&route.grant));
        routes.insert(id.clone(), Arc::new(route));
        Ok(format!("http://{addr}/mcp/{id}"))
    }

    fn route_of(&self, id: &str) -> Option<Arc<Route>> {
        lock(&self.routes).get(id).cloned()
    }

    /// The access token to send for `route`: renewed when it is about to expire, or when the
    /// server refused `refused` (the token last sent) and no one has renewed it since.
    pub(crate) async fn token(
        &self,
        route: &Route,
        refused: Option<&Secret>,
    ) -> Result<Secret, String> {
        let slot = self.slot(&route.place);
        let mut held = slot.lock().await;
        if held.is_none() {
            self.not_refused(&route.place)?;
            let (stores, place) = (self.stores(route.homes.clone()), route.place.clone());
            match off(move || stores.read(&place, false)).await {
                Ok(Some(credential))
                    if credential.usable() && !self.is_dead(&route.place, &credential) =>
                {
                    *held = Some(credential);
                }
                Ok(_) => {
                    self.know(&route.place, Probe::Absent);
                    return Err(format!(
                        "{} holds no usable sign-in for it now",
                        route.place.owner()
                    ));
                }
                Err(err) => {
                    self.read_failed(&route.place, &err);
                    return Err(format!(
                        "{}'s sign-in couldn't be read: {err}",
                        route.place.owner()
                    ));
                }
            }
        }
        let current = held.as_ref().map(|credential| {
            (
                refused.is_some_and(|token| *token == credential.access_token)
                    || credential.expires_within(RENEW_WITHIN),
                credential.access_token.clone(),
            )
        });
        match current {
            Some((false, token)) => Ok(token),
            _ => {
                let renewed = self.renew(route, held.take()).await?;
                let token = renewed.access_token.clone();
                *held = Some(renewed);
                Ok(token)
            }
        }
    }

    /// Renews `ours` under the owner's lock: the store's own when the owner renewed it
    /// meanwhile, else refreshed and written back to the store.
    async fn renew(&self, route: &Route, ours: Option<Credential>) -> Result<Credential, String> {
        let place = &route.place;
        self.not_refused(place)?;
        let stores = Arc::new(self.stores(route.homes.clone()));
        let taken = {
            let (stores, place) = (stores.clone(), place.clone());
            off(move || {
                let locks = stores.renewal_lock(&place)?;
                Ok((locks, stores.read(&place, false)?))
            })
            .await
        };
        let (locks, now) = taken.map_err(|err| {
            self.read_failed(place, &err);
            format!("{}'s sign-in couldn't be read: {err}", place.owner())
        })?;
        let Some(now) = now.filter(|now| now.usable() && !self.is_dead(place, now)) else {
            drop(locks);
            self.forget(place);
            return Err(format!(
                "{} holds no usable sign-in for it now",
                place.owner()
            ));
        };
        let renewed_by_owner = ours
            .as_ref()
            .is_some_and(|ours| ours.access_token != now.access_token);
        if renewed_by_owner && !now.expires_within(RENEW_WITHIN) {
            return Ok(now);
        }
        let now = now.with_client(route.client.as_ref());
        // The resource the tokens are for goes with the refresh (RFC 8707), as with the
        // sign-in; the stores don't keep it.
        let (endpoint, resource) = match (
            oauth::discover(&self.http, &route.url, &route.headers).await,
            &now.token_endpoint,
        ) {
            (Ok(found), stored) => (
                stored.clone().unwrap_or(found.auth.token_endpoint),
                Some(found.resource),
            ),
            (Err(_), Some(stored)) => (stored.clone(), None),
            (Err(err), None) => return Err(err.to_string()),
        };
        let renewed = match oauth::refresh(&self.http, &endpoint, &now, resource.as_deref()).await {
            Ok(renewed) => renewed,
            Err(err) => {
                if err == oauth::OAuthError::Refused {
                    self.bury(place, &now);
                } else {
                    self.forget(place);
                }
                tracing::warn!(owner = place.owner(), reason = %err, "an MCP sign-in wasn't renewed");
                return Err(err.to_string());
            }
        };
        let written = {
            let (place, now, renewed) = (place.clone(), now.clone(), renewed.clone());
            off(move || stores.write_back(&place, &now, &renewed)).await
        };
        drop(locks);
        match written {
            Ok(()) => {
                self.know(place, Probe::Usable);
                Ok(renewed)
            }
            Err(err) => {
                tracing::warn!(owner = place.owner(), reason = %err, "a renewed MCP sign-in couldn't be written back; it is no longer reused");
                self.bury(place, &now);
                Err(format!(
                    "{}'s sign-in changed while it was renewed",
                    place.owner()
                ))
            }
        }
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

    pub(crate) fn homes(&self) -> &Homes {
        &self.stores.homes
    }
}
