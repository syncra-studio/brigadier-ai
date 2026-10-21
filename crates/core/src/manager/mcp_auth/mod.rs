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
//!   may show the Keychain's prompt once.
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

use std::collections::HashMap;
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
    Failed {
        reason: String,
    },
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
    routes: Mutex<HashMap<String, Arc<Route>>>,
    proxy: tokio::sync::OnceCell<SocketAddr>,
    sign_ins: Mutex<HashMap<String, McpSignInStatus>>,
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

    /// What `place` holds: known within [`KNOWN_FOR`], a sign-in in use, else read now.
    fn probe(&self, stores: &Stores, place: &Place, quiet: bool) -> Probe {
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
                Probe::Absent
            }
            Err(err) => {
                self.forget(place);
                match err {
                    StoreError::Busy => {}
                    StoreError::Denied => {
                        tracing::info!(
                            owner = place.owner(),
                            "reading a stored MCP sign-in was refused"
                        );
                    }
                    StoreError::Failed(reason) => {
                        tracing::warn!(owner = place.owner(), %reason, "a stored MCP sign-in couldn't be read");
                    }
                }
                Probe::Unknown
            }
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
                    self.forget(&route.place);
                    return Err(format!(
                        "{}'s sign-in couldn't be read: {err:?}",
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
            self.forget(place);
            format!("{}'s sign-in couldn't be read: {err:?}", place.owner())
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
        let endpoint = match &now.token_endpoint {
            Some(endpoint) => endpoint.clone(),
            None => {
                oauth::discover(&self.http, &route.url, &route.headers)
                    .await
                    .map_err(|err| err.to_string())?
                    .auth
                    .token_endpoint
            }
        };
        let renewed = match oauth::refresh(&self.http, &endpoint, &now, None).await {
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
                tracing::warn!(owner = place.owner(), reason = ?err, "a renewed MCP sign-in couldn't be written back; it is no longer reused");
                self.bury(place, &now);
                Err(format!(
                    "{}'s sign-in changed while it was renewed",
                    place.owner()
                ))
            }
        }
    }

    /// A new sign-in in Brigadier, pending: its id.
    pub(crate) fn begin_sign_in(&self) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        lock(&self.sign_ins).insert(id.clone(), McpSignInStatus::Pending);
        id
    }

    /// How sign-in `id` went; `None` for one Brigadier doesn't know.
    pub(crate) fn sign_in_status(&self, id: &str) -> Option<McpSignInStatus> {
        lock(&self.sign_ins).get(id).cloned()
    }

    /// Runs sign-in `id` to `server` in the user's browser (`open`), then keeps the sign-in in
    /// Brigadier's own Keychain item.
    pub(crate) async fn sign_in(
        &self,
        id: &str,
        server: &Server,
        homes: Homes,
        open: &(dyn Fn(&str) + Send + Sync),
    ) {
        let outcome = self.signed_in(server, homes, open).await;
        let status = match outcome {
            Ok(()) => McpSignInStatus::SignedIn,
            Err(reason) => {
                tracing::warn!(server = %server.name, %reason, "signing in to an MCP server failed");
                McpSignInStatus::Failed { reason }
            }
        };
        lock(&self.sign_ins).insert(id.to_owned(), status);
    }

    async fn signed_in(
        &self,
        server: &Server,
        homes: Homes,
        open: &(dyn Fn(&str) + Send + Sync),
    ) -> Result<(), String> {
        let (Transport::Http { url, headers }, Some(place)) = (
            &server.transport,
            Place::of(Owner::Brigadier, server, CodexStore::default()),
        ) else {
            return Err("only a remote server signs in".into());
        };
        let credential = oauth::sign_in(&self.http, url, headers, server.oauth.as_ref(), open)
            .await
            .map_err(|err| err.to_string())?;
        let stores = self.stores(homes);
        let saved = {
            let place = place.clone();
            off(move || match &place {
                Place::Brigadier { identity, url } => stores.save_own(identity, url, &credential),
                _ => Err(StoreError::Failed("not Brigadier's own".into())),
            })
            .await
        };
        saved.map_err(|err| format!("the sign-in couldn't be kept in the Keychain: {err:?}"))?;
        lock(&self.dead).remove(&place);
        self.forget(&place);
        Ok(())
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

    pub(crate) fn homes(&self) -> &Homes {
        &self.stores.homes
    }
}
