//! Linux: the password Chromium keeps in the desktop keyring, over the Secret Service D-Bus API
//! that gnome-keyring and others serve (the API libsecret and `secret-tool` also use).
//!
//! It only ever reads an already unlocked item. A locked keyring is reported, not unlocked: that
//! would mean waiting on a desktop prompt, and the user can unlock it and try again.

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;
use zbus::blocking::{Connection, connection};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Type};

use crate::SecretError;

const SERVICE: &str = "org.freedesktop.secrets";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const SERVICE_INTERFACE: &str = "org.freedesktop.Secret.Service";
const ITEM_INTERFACE: &str = "org.freedesktop.Secret.Item";
/// A stuck keyring daemon must not hang an import.
const TIMEOUT: Duration = Duration::from_secs(10);

/// The spec's `Secret` struct, the one thing `GetSecret` returns.
#[derive(Debug, Deserialize, Type)]
struct Secret {
    _session: OwnedObjectPath,
    _parameters: Vec<u8>,
    value: Vec<u8>,
    _content_type: String,
}

/// The password stored under `application=<application>`, as `secret-tool lookup` would find it.
#[cfg(target_os = "linux")]
pub(crate) fn lookup(application: &str) -> Result<Vec<u8>, SecretError> {
    let connection = connection::Builder::session()
        .and_then(|builder| builder.method_timeout(TIMEOUT).build())
        .map_err(|err| {
            tracing::debug!(%err, "no session bus");
            SecretError::NoStore
        })?;
    lookup_on(&connection, application)
}

fn lookup_on(connection: &Connection, application: &str) -> Result<Vec<u8>, SecretError> {
    let reply = connection
        .call_method(
            Some(SERVICE),
            SERVICE_PATH,
            Some(SERVICE_INTERFACE),
            "OpenSession",
            &("plain", zbus::zvariant::Value::from("")),
        )
        .map_err(failure)?;
    let (_, session): (OwnedValue, OwnedObjectPath) =
        reply.body().deserialize().map_err(failure)?;

    let attributes = HashMap::from([("application", application)]);
    let reply = connection
        .call_method(
            Some(SERVICE),
            SERVICE_PATH,
            Some(SERVICE_INTERFACE),
            "SearchItems",
            &(attributes,),
        )
        .map_err(failure)?;
    let (unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) =
        reply.body().deserialize().map_err(failure)?;
    let Some(item) = unlocked.first() else {
        return Err(if locked.is_empty() {
            SecretError::NotFound
        } else {
            SecretError::Locked
        });
    };

    let reply = connection
        .call_method(
            Some(SERVICE),
            item.as_str(),
            Some(ITEM_INTERFACE),
            "GetSecret",
            &(&session,),
        )
        .map_err(failure)?;
    let secret: Secret = reply.body().deserialize().map_err(failure)?;
    if secret.value.is_empty() {
        return Err(SecretError::NotFound);
    }
    Ok(secret.value)
}

/// What a D-Bus failure means to the user. No service to call means no keyring on this machine.
fn failure(err: zbus::Error) -> SecretError {
    if let zbus::Error::MethodError(name, ..) = &err {
        match name.as_str() {
            "org.freedesktop.DBus.Error.ServiceUnknown"
            | "org.freedesktop.DBus.Error.NameHasNoOwner" => return SecretError::NoStore,
            "org.freedesktop.Secret.Error.IsLocked" => return SecretError::Locked,
            "org.freedesktop.Secret.Error.NoSuchObject" => return SecretError::NotFound,
            "org.freedesktop.DBus.Error.AccessDenied" => return SecretError::Denied,
            _ => {}
        }
    }
    SecretError::Other(err.to_string())
}

/// These run the real client over a socket pair against a fake service with the spec's
/// signatures, so they need no keyring, no session bus and no Linux.
#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::net::UnixStream;

    use serde::Serialize;
    use zbus::fdo;
    use zbus::zvariant::{ObjectPath, Value};

    use super::*;

    #[derive(Serialize, Type)]
    struct ReplySecret(OwnedObjectPath, Vec<u8>, Vec<u8>, String);

    fn path(text: &str) -> OwnedObjectPath {
        ObjectPath::try_from(text).unwrap().into()
    }

    struct FakeService {
        unlocked: Vec<OwnedObjectPath>,
        locked: Vec<OwnedObjectPath>,
    }

    #[zbus::interface(name = "org.freedesktop.Secret.Service")]
    impl FakeService {
        fn open_session(
            &self,
            algorithm: &str,
            _input: Value<'_>,
        ) -> fdo::Result<(OwnedValue, OwnedObjectPath)> {
            if algorithm != "plain" {
                return Err(fdo::Error::NotSupported(algorithm.to_owned()));
            }
            let output = OwnedValue::try_from(Value::from("")).unwrap();
            Ok((output, path("/org/freedesktop/secrets/session/s1")))
        }

        fn search_items(
            &self,
            attributes: HashMap<String, String>,
        ) -> (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) {
            if attributes.get("application").map(String::as_str) == Some("chrome") {
                (self.unlocked.clone(), self.locked.clone())
            } else {
                (Vec::new(), Vec::new())
            }
        }
    }

    struct FakeItem {
        value: Vec<u8>,
    }

    #[zbus::interface(name = "org.freedesktop.Secret.Item")]
    impl FakeItem {
        fn get_secret(&self, session: OwnedObjectPath) -> ReplySecret {
            ReplySecret(
                session,
                Vec::new(),
                self.value.clone(),
                "text/plain".to_owned(),
            )
        }
    }

    /// The fake must answer what a real keyring does, or these tests prove nothing.
    #[test]
    fn the_secret_has_the_specs_signature() {
        assert_eq!(Secret::SIGNATURE.to_string(), "(oayays)");
        assert_eq!(ReplySecret::SIGNATURE.to_string(), "(oayays)");
    }

    /// A client connected to a fake service holding one item, unlocked or locked.
    // `unix_stream` would only stop building if something turned on zbus's `tokio` feature, and
    // nothing does; its replacement needs another dependency just for these tests.
    #[allow(deprecated)]
    fn client(value: &[u8], locked: bool) -> (Connection, Connection) {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let item = path("/org/freedesktop/secrets/collection/login/1");
        let service = FakeService {
            unlocked: if locked {
                Vec::new()
            } else {
                vec![item.clone()]
            },
            locked: if locked {
                vec![item.clone()]
            } else {
                Vec::new()
            },
        };
        let value = value.to_vec();
        let server = std::thread::spawn(move || {
            connection::Builder::unix_stream(theirs)
                .server(zbus::Guid::generate())
                .unwrap()
                .p2p()
                .serve_at(SERVICE_PATH, service)
                .unwrap()
                .serve_at(item.as_str(), FakeItem { value })
                .unwrap()
                .build()
                .unwrap()
        });
        let client = connection::Builder::unix_stream(ours)
            .p2p()
            .method_timeout(TIMEOUT)
            .build()
            .unwrap();
        (client, server.join().unwrap())
    }

    #[test]
    fn an_unlocked_item_gives_its_secret() {
        let (client, _server) = client(b"hunter2", false);
        assert_eq!(lookup_on(&client, "chrome").unwrap(), b"hunter2");
    }

    #[test]
    fn a_locked_keyring_is_reported_not_unlocked() {
        let (client, _server) = client(b"hunter2", true);
        assert_eq!(lookup_on(&client, "chrome"), Err(SecretError::Locked));
    }

    #[test]
    fn no_entry_for_this_browser() {
        let (client, _server) = client(b"hunter2", false);
        assert_eq!(lookup_on(&client, "brave"), Err(SecretError::NotFound));
    }

    #[test]
    fn an_empty_secret_is_no_entry() {
        let (client, _server) = client(b"", false);
        assert_eq!(lookup_on(&client, "chrome"), Err(SecretError::NotFound));
    }

    #[test]
    fn dbus_errors_become_cross_os_states() {
        let method_error = |name: &'static str| {
            let reply = zbus::Message::method_call("/", "x")
                .unwrap()
                .build(&())
                .unwrap();
            let error = zbus::Message::error(&reply.header(), name)
                .unwrap()
                .build(&())
                .unwrap();
            zbus::Error::from(error)
        };
        assert_eq!(
            failure(method_error("org.freedesktop.DBus.Error.ServiceUnknown")),
            SecretError::NoStore
        );
        assert_eq!(
            failure(method_error("org.freedesktop.DBus.Error.NameHasNoOwner")),
            SecretError::NoStore
        );
        assert_eq!(
            failure(method_error("org.freedesktop.Secret.Error.IsLocked")),
            SecretError::Locked
        );
        assert!(matches!(
            failure(zbus::Error::InterfaceNotFound),
            SecretError::Other(_)
        ));
    }
}
